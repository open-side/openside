"""Run real daemon/sidecar/browser tasks and export artifacts in the BU Bench V1 result format."""

from __future__ import annotations

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import threading
import time
from contextlib import contextmanager
from datetime import UTC, datetime
from pathlib import Path

import httpx
import psutil
from jsonschema import Draft202012Validator, FormatChecker

from fixtures import DOWNLOAD, OTP, TABLE, TASKS, TICKET_SUMMARY, fixture_server

REPO = Path(__file__).resolve().parent.parent
SCHEMA = REPO / "docs/specs/bench-schema.json"
SUITE = "openside-local-web-v1"


def validator() -> Draft202012Validator:
    schema = json.loads(SCHEMA.read_text())
    Draft202012Validator.check_schema(schema)
    return Draft202012Validator(schema, format_checker=FormatChecker())


def validate_result(result: dict) -> None:
    validator().validate(result)


def write_json(path: Path, value) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")


def discover_browser() -> Path:
    explicit = os.environ.get("OPENSIDE_PATCHED_BROWSER") or os.environ.get("OPENSIDE_BROWSER")
    if explicit:
        found = shutil.which(explicit)
        if found:
            return Path(found).resolve()
        raise RuntimeError("Configured OPENSIDE_PATCHED_BROWSER/OPENSIDE_BROWSER is not executable")
    for name in ("chromium", "chromium-browser", "google-chrome", "google-chrome-stable", "chrome"):
        found = shutil.which(name)
        if found:
            return Path(found).resolve()
    if sys.platform == "darwin":
        candidate = Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome")
        if candidate.is_file():
            return candidate
    raise RuntimeError("Chromium is unavailable; install it or set OPENSIDE_BROWSER")


class Descendants:
    """Remember children before daemon/sidecar exit reparents browser processes."""

    def __init__(self, process: subprocess.Popen):
        self.process = psutil.Process(process.pid)
        self.known: dict[int, psutil.Process] = {}
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self.watch, daemon=True)
        self.thread.start()

    def capture(self):
        try:
            children = self.process.children(recursive=True)
        except psutil.NoSuchProcess:
            return
        for child in children:
            self.known[child.pid] = child

    def watch(self):
        while not self.stop.wait(0.02):
            self.capture()

    def cleanup(self, daemon: subprocess.Popen) -> list[int]:
        self.capture()
        self.stop.set()
        self.thread.join(timeout=5)
        if daemon.poll() is None:
            daemon.send_signal(signal.SIGTERM)
            try:
                daemon.wait(timeout=8)
            except subprocess.TimeoutExpired:
                daemon.kill()
                daemon.wait(timeout=8)
        children = list(self.known.values())
        for child in children:
            try:
                child.kill()
            except psutil.NoSuchProcess:
                pass
        _, alive = psutil.wait_procs(children, timeout=8)
        # Reparented zombies are already dead; their system reaper owns waitpid.
        survivors = []
        for child in alive:
            try:
                if child.status() != psutil.STATUS_ZOMBIE:
                    survivors.append(child.pid)
            except psutil.NoSuchProcess:
                pass
        if survivors:
            raise RuntimeError(f"Benchmark processes survived cleanup: {survivors}")
        return [daemon.pid, *self.known]


@contextmanager
def daemon_process(binary: Path, browser: Path, python: Path, live_model: str | None, origin: str):
    if os.name != "posix":
        raise RuntimeError("The real daemon benchmark currently requires Unix sockets (Linux/macOS)")
    with tempfile.TemporaryDirectory(prefix="openside-bench-") as temporary:
        root = Path(temporary)
        for directory in ("home", "config/openside", "data", "state", "cache", "run", "tmp", "workspace"):
            (root / directory).mkdir(parents=True, exist_ok=True)
        (root / "run").chmod(0o700)
        script = root / "script.json"
        write_json(script, {"responses": []})
        profile = "ultrabrowse" if live_model else "full"
        model = live_model or "script"
        config = (
            "[agent]\ncommand = " + json.dumps([str(python), "-m", "openside_agent"])
            + ('\norchestration = "single"\n' if not live_model else "\n")
            + f"\nprofile = {json.dumps(profile)}\n"
            + "[models]\ndefault = " + json.dumps(model)
            + "\n[policy]\nmode = \"full\"\n"
            + "allow_origins = " + json.dumps([origin]) + "\n"
            + "[browser]\nexecutable = " + json.dumps(str(browser)) + "\n"
        )
        if live_model:
            provider = live_model.split(":", 1)[0]
            config += "[credentials.order]\n" + json.dumps(provider) + ' = ["env"]\n'
        (root / "config/openside/config.toml").write_text(config)
        environment = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": str(root / "home"),
            "XDG_CONFIG_HOME": str(root / "config"),
            "XDG_DATA_HOME": str(root / "data"),
            "XDG_STATE_HOME": str(root / "state"),
            "XDG_CACHE_HOME": str(root / "cache"),
            "XDG_RUNTIME_DIR": str(root / "run"),
            "TMPDIR": str(root / "tmp"),
        }
        if live_model:
            # Live mode is explicit; no broker, CLI-auth stores or user configuration are inherited.
            for name in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GOOGLE_API_KEY", "GEMINI_API_KEY", "OPENROUTER_API_KEY"):
                if name in os.environ:
                    environment[name] = os.environ[name]
        else:
            environment["OPENSIDE_AGENT_SCRIPT"] = str(script)
        socket = root / "run/opensided.sock"
        with (root / "daemon.stdout").open("wb") as stdout, (root / "daemon.stderr").open("wb") as stderr:
            process = subprocess.Popen(
                [str(binary), "--data-dir", str(root / "data"), "--socket", str(socket)],
                env=environment, cwd=root / "workspace", stdin=subprocess.DEVNULL,
                stdout=stdout, stderr=stderr, start_new_session=True,
            )
            descendants = Descendants(process)
            try:
                transport = httpx.HTTPTransport(uds=str(socket), retries=0)
                with httpx.Client(transport=transport, base_url="http://localhost", timeout=120, trust_env=False) as client:
                    deadline = time.monotonic() + 30
                    while True:
                        if process.poll() is not None:
                            raise RuntimeError("Daemon exited: " + (root / "daemon.stderr").read_text())
                        try:
                            if client.get("/v1/health").is_success:
                                break
                        except httpx.TransportError:
                            pass
                        if time.monotonic() >= deadline:
                            raise RuntimeError("Daemon startup deadline: " + (root / "daemon.stderr").read_text())
                        time.sleep(0.02)
                    yield root, client, script, descendants
            finally:
                descendants.cleanup(process)


def request(client: httpx.Client, method: str, path: str, body=None):
    response = client.request(method, path, json=body)
    response.raise_for_status()
    return response.json() if response.content else None


def collect_turn(client: httpx.Client, session: str, task, instruction: str,
                 descendants: Descendants, events: list[dict]):
    tickets = []
    resolved = 0
    started = time.monotonic()
    with client.stream("GET", f"/v1/sessions/{session}/events", headers={"Last-Event-ID": "0"}) as response:
        response.raise_for_status()
        request(client, "POST", f"/v1/sessions/{session}/messages", {
            "message_id": f"bench-{task.task_id}", "text": instruction, "mode": "now",
        })
        for line in response.iter_lines():
            if time.monotonic() - started > 120:
                raise TimeoutError(f"Task {task.task_id} exceeded 120 seconds")
            if not line.startswith("data:"):
                continue
            event = json.loads(line[5:].strip())
            events.append(event)
            descendants.capture()
            if event["kind"] == "ticket.opened":
                tickets.append(event["ticket"])
            if event["kind"] != "session.settled":
                continue
            if tickets:
                for ticket in tickets:
                    if (task.task_id != "login" or ticket["kind"] != "info" or resolved
                            or ticket["summary"] != TICKET_SUMMARY):
                        raise RuntimeError("Unexpected ticket: refusing to auto-approve unrelated work")
                    request(client, "POST", f"/v1/tickets/{ticket['id']}/resolve", {"value": OTP})
                    resolved += 1
                tickets.clear()
                continue
            if any(item["kind"] == "assistant.final" for item in events):
                return events, resolved
            raise RuntimeError(f"Task settled without an assistant reply: {events}")
    raise RuntimeError("SSE closed before the task settled")


def grade(task, state, root: Path, events: list[dict], resolved: int, live: bool) -> tuple[bool, str]:
    errors = [event for event in events if event["kind"] == "tool.ended" and event["is_error"]]
    completions = [event for event in events if event["kind"] == "message.completed"]
    if errors or any(event["outcome"] in ("failed", "aborted") for event in completions):
        return False, "A real daemon tool or sidecar turn failed"
    if not live and not any(event["kind"] == "assistant.final" and event["text"] == f"Completed {task.task_id}." for event in events):
        return False, "The scripted model did not reach its final response"
    with state.lock:
        if task.task_id == "form-fill":
            success = state.submitted.get("/form-fill/submitted") == {"name": "Ada", "city": "Oslo"}
        elif task.task_id == "navigation":
            visits = state.visits.get("navigation", [])
            expected = ["/navigation/", "/navigation/step-two", "/navigation/finish"]
            success = [path for path in visits if path in expected] == expected
        elif task.task_id == "table-extraction":
            try:
                success = json.loads(state.submitted.get("/table-extraction/submitted", {}).get("rows", "null")) == TABLE
            except json.JSONDecodeError:
                success = False
        elif task.task_id == "login":
            success = state.login_complete and resolved == 1
        else:
            success = any(path.read_bytes() == DOWNLOAD for path in root.rglob("report.csv") if path.is_file())
    return success, ("Fixture state and actual daemon execution satisfy the task" if success
                     else "Fixture state does not satisfy the task")


def metrics(events: list[dict], state, task_id: str, resolved: int, live: bool) -> dict:
    usage = [event for event in events if event["kind"] == "usage"]
    with state.lock:
        browser_bytes = state.browser_bytes.get(task_id, 0)
    return {
        "suite": SUITE, "mode": "live" if live else "scripted",
        "orchestration": "front" if live else "single", "grader": "fixture-state-v1",
        "steps": sum(event["kind"] == "tool.started" for event in events),
        "tokens": {
            "input": sum(event["input_tokens"] for event in usage),
            "output": sum(event["output_tokens"] for event in usage),
            "source": "provider-usage" if live else "scripted-function-model-estimate",
        },
        "browserBytes": browser_bytes, "ticketsResolved": resolved,
    }


def run_suite(binary: Path, browser: Path, python: Path, output: Path, live_model: str | None = None) -> dict:
    if live_model and ":" not in live_model:
        raise ValueError("OPENSIDE_LIVE_MODEL must be a provider-qualified selector")
    if output.exists() and any(output.iterdir()):
        raise ValueError("Use a new or empty results directory; previous measurements are never overwritten")
    output.mkdir(parents=True, exist_ok=True)
    live = bool(live_model)
    provider, model_id = live_model.split(":", 1) if live_model else ("scripted", "script")
    selection = {"provider": provider, "modelId": model_id, "thinkingLevel": "high" if live else "off", "fastMode": False}
    results = []
    suite_start = time.monotonic()
    with fixture_server() as (origin, state), daemon_process(binary, browser, python, live_model, origin) as (root, client, script, descendants):
        for task in TASKS:
            write_json(script, task.script(origin))
            session = request(client, "POST", "/v1/sessions", {
                "title": f"Benchmark: {task.task_id}", "mode": "full", "workspace": str(root / "workspace"),
            })["id"]
            instruction = task.instruction.format(origin=origin)
            if live:
                instruction = "This is a local synthetic fixture with fake credentials, not a real account. " + instruction
            task_start = time.monotonic()
            events = []
            resolved = 0
            error = None
            try:
                events, resolved = collect_turn(client, session, task, instruction, descendants, events)
                if task.task_id == "download":
                    deadline = time.monotonic() + 5
                    while not any(path.read_bytes() == DOWNLOAD for path in root.rglob("report.csv") if path.is_file()) and time.monotonic() < deadline:
                        time.sleep(0.05)
            except (httpx.HTTPError, TimeoutError, RuntimeError) as failure:
                error = str(failure)
                resolved = sum(event["kind"] == "ticket.resolved" for event in events)
                request(client, "POST", f"/v1/sessions/{session}/stop", {})
            duration = round((time.monotonic() - task_start) * 1000, 3)
            grade_start = time.monotonic()
            passed, reasoning = (False, error) if error else grade(task, state, root, events, resolved, live)
            grade_duration = round((time.monotonic() - grade_start) * 1000, 3)
            folder = output / task.task_id
            folder.mkdir()
            stdout = "\n".join(event["text"] for event in events if event["kind"] == "assistant.final")
            (folder / "stdout.txt").write_text(stdout + "\n")
            (folder / "stderr.txt").write_text((root / "daemon.stderr").read_text())
            (folder / "events.jsonl").write_text("".join(json.dumps(event) + "\n" for event in events))
            (folder / "trajectory.txt").write_text("".join(
                f"{event['seq']} {event['kind']} {json.dumps(event.get('result', event.get('args', {})))}\n"
                for event in events if event["kind"] in ("tool.started", "tool.ended")
            ))
            result = {
                "task_id": task.task_id, "category": task.category, "task": instruction,
                "outcome": "error" if error else "success" if passed else "failure",
                "graderVerdict": "error" if error else "pass" if passed else "fail",
                "graderReasoning": reasoning, "agentStatus": "error" if error else "success" if passed else "failure",
                "sessionId": session, "model": selection, "provider": provider,
                "durationMs": duration, "graderDurationMs": grade_duration,
                "tabCount": len({event["tab_id"] for event in events if event["kind"] == "tab.opened"}),
                "messageCount": sum(event["kind"] in ("message.queued", "assistant.final") for event in events),
                "artifacts": {"stdout": "stdout.txt", "stderr": "stderr.txt", "cliEvents": "events.jsonl", "trajectory": "trajectory.txt", "result": "result.json"},
                "openside": metrics(events, state, task.task_id, resolved, live),
            }
            validate_result(result)
            write_json(folder / "result.json", result)
            results.append(result)
            print(f"{task.task_id}: {result['graderVerdict']} ({duration:.0f} ms, {result['openside']['steps']} steps)", flush=True)
            request(client, "DELETE", f"/v1/sessions/{session}")
    summary = summarize(results, selection, round((time.monotonic() - suite_start) * 1000, 3))
    validate_result(summary)
    write_json(output / "run_summary.json", summary)
    print(f"{summary['passed']}/{summary['total']} passed; validated results: {output}", flush=True)
    return summary


def summarize(results: list[dict], selection: dict, total_duration: float) -> dict:
    by_category = {}
    counts = {"passed": 0, "failed": 0, "impossible": 0, "errors": 0}
    verdict_key = {"pass": "passed", "fail": "failed", "impossible": "impossible", "error": "errors"}
    extension = {"suite": SUITE, "mode": results[0]["openside"]["mode"],
                 "orchestration": results[0]["openside"]["orchestration"],
                 "grader": "fixture-state-v1", "steps": 0,
                 "tokens": {"input": 0, "output": 0, "source": results[0]["openside"]["tokens"]["source"]},
                 "browserBytes": 0, "ticketsResolved": 0}
    for result in results:
        category = by_category.setdefault(result["category"], {"total": 0, **dict.fromkeys(counts, 0)})
        key = verdict_key[result["graderVerdict"]]
        category["total"] += 1
        category[key] += 1
        counts[key] += 1
        for field in ("steps", "browserBytes", "ticketsResolved"):
            extension[field] += result["openside"][field]
        for field in ("input", "output"):
            extension["tokens"][field] += result["openside"]["tokens"][field]
    total = len(results)
    return {
        "total": total, "ran": total, **counts, "skipped": 0, "byCategory": by_category,
        "passRate": counts["passed"] / total,
        "passRateExcludingImpossible": counts["passed"] / (total - counts["impossible"]) if total > counts["impossible"] else 0,
        "avgDurationMs": sum(result["durationMs"] for result in results) / total,
        "totalDurationMs": total_duration, "model": selection["modelId"], "provider": selection["provider"],
        "thinkingLevel": selection["thinkingLevel"], "fastMode": False, "concurrency": 1,
        "timestamp": datetime.now(UTC).isoformat(), "openside": extension,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--daemon", type=Path, required=True)
    parser.add_argument("--python", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=REPO / "bench/results" / datetime.now(UTC).strftime("%Y%m%dT%H%M%S%fZ"))
    arguments = parser.parse_args()
    try:
        # Resolving a virtualenv interpreter symlink selects the base Python without its packages.
        summary = run_suite(arguments.daemon.resolve(), discover_browser(), arguments.python.absolute(), arguments.output.resolve(), os.environ.get("OPENSIDE_LIVE_MODEL"))
    except (ValueError, RuntimeError, OSError, httpx.HTTPError) as error:
        print(f"openside-bench: {error}", file=sys.stderr)
        return 2
    return 0 if summary["passed"] == summary["total"] else 1


if __name__ == "__main__":
    raise SystemExit(main())

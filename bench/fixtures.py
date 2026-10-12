"""Loopback-only tasks whose graders inspect state, not the agent's success claim."""

from __future__ import annotations

import json
import threading
from contextlib import contextmanager
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlsplit

DOWNLOAD = b"item,quantity\napples,3\npears,5\n"
TABLE = ["Apples 3", "Pears 5", "Oranges 2"]
OTP = "123456"
TICKET_SUMMARY = "Resolve the synthetic 2FA fixture challenge"


@dataclass
class FixtureState:
    browser_bytes: dict[str, int] = field(default_factory=dict)
    visits: dict[str, list[str]] = field(default_factory=dict)
    submitted: dict[str, dict[str, str]] = field(default_factory=dict)
    login_complete: bool = False
    lock: threading.Lock = field(default_factory=threading.Lock)


@dataclass(frozen=True)
class Task:
    task_id: str
    category: str
    instruction: str
    codes: tuple[str, ...]
    ticket_after: int | None = None

    def script(self, origin: str) -> dict:
        responses = []
        for index, code in enumerate(self.codes):
            if self.ticket_after == index:
                responses.append({"parts": [{
                    "type": "tool_call", "name": "request_human_step", "id": "fixture-2fa",
                    "args": {"kind": "info", "summary": TICKET_SUMMARY},
                }]})
            responses.append({"parts": [{
                "type": "tool_call", "name": "browser_repl", "id": f"step-{index}",
                "args": {"code": code.replace("FIXTURE_ORIGIN", origin)},
            }]})
        responses.append({"parts": [
            {"type": "wait_steer", "timeout_ms": 500},
            {"type": "text", "text": f"Completed {self.task_id}."},
        ]})
        return {"responses": responses}


TASKS = (
    Task(
        "form-fill", "InteractionTests",
        "Open {origin}/form-fill/, enter name Ada and city Oslo, and submit the form.",
        (
            "await page.goto('FIXTURE_ORIGIN/form-fill/'); await page.snapshot()",
            "await page.getByLabel('Name').fill('Ada'); "
            "await page.getByLabel('City').fill('Oslo')",
            "await page.getByRole('button', {name: 'Submit form'}).click(); "
            "await page.waitForURL('**/submitted'); await page.snapshot()",
        ),
    ),
    Task(
        "navigation", "OM2W2",
        "Open {origin}/navigation/ and follow Continue then Finish to the final page.",
        (
            "await page.goto('FIXTURE_ORIGIN/navigation/'); await page.snapshot()",
            "await page.getByRole('link', {name: 'Continue'}).click(); "
            "await page.waitForURL('**/step-two'); await page.snapshot()",
            "await page.getByRole('link', {name: 'Finish'}).click(); "
            "await page.waitForURL('**/finish'); await page.snapshot()",
        ),
    ),
    Task(
        "table-extraction", "WebBenchREAD",
        "Open {origin}/table-extraction/, read each inventory row, and submit the "
        "rows as a JSON array of strings in the Extracted rows field.",
        (
            "await page.goto('FIXTURE_ORIGIN/table-extraction/'); await page.snapshot()",
            "const rows = await page.locator('tbody tr').allInnerTexts(); "
            "await page.getByLabel('Extracted rows').fill(JSON.stringify(rows)); rows",
            "await page.getByRole('button', {name: 'Save extraction'}).click(); "
            "await page.waitForURL('**/submitted'); await page.snapshot()",
        ),
    ),
    Task(
        "login", "InteractionTests",
        "Open {origin}/login/, sign in as demo with password fixture-only, request "
        "a non-secret synthetic challenge ticket using request_human_step with kind "
        f"info and summary '{TICKET_SUMMARY}', then fill its answer in Verification code.",
        (
            "await page.goto('FIXTURE_ORIGIN/login/'); await page.snapshot()",
            "await page.getByLabel('Username').fill('demo'); "
            "await page.getByLabel('Password').fill('fixture-only'); "
            "await page.getByRole('button', {name: 'Sign in'}).click(); "
            "await page.waitForURL('**/otp'); await page.snapshot()",
            f"await page.getByLabel('Verification code').fill('{OTP}'); "
            "await page.getByRole('button', {name: 'Verify code'}).click(); "
            "await page.waitForURL('**/verified'); await page.snapshot()",
        ),
        ticket_after=2,
    ),
    Task(
        "download", "InteractionTests",
        "Open {origin}/download/ and download report.csv using the Get report link.",
        (
            "await page.goto('FIXTURE_ORIGIN/download/'); await page.snapshot()",
            "await page.getByRole('link', {name: 'Get report'}).click()",
        ),
    ),
)


def page(body: str) -> bytes:
    return ("<!doctype html><html><head><meta charset='utf-8'>"
            "<link rel='icon' href='data:,'><title>Local benchmark</title>"
            f"</head><body>{body}</body></html>").encode()


def handler_for(state: FixtureState):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def reply(self, status: int, body: bytes, task: str, headers=()):
            self.send_response(status)
            if not any(name.lower() == "content-type" for name, _ in headers):
                self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Security-Policy", "default-src 'self'; img-src data:; form-action 'self'")
            for name, value in headers:
                self.send_header(name, value)
            self.end_headers()
            self.wfile.write(body)
            with state.lock:
                state.browser_bytes[task] = state.browser_bytes.get(task, 0) + len(body)

        def do_GET(self):
            path = urlsplit(self.path).path
            task = path.strip("/").split("/")[0]
            with state.lock:
                state.visits.setdefault(task, []).append(path)
            pages = {
                "/form-fill/": "<h1>Contact form</h1><form method='post' action='/form-fill/submitted'>"
                "<label>Name<input name='name'></label><label>City<input name='city'></label>"
                "<button>Submit form</button></form>",
                "/navigation/": "<h1>Start</h1><a href='/navigation/step-two'>Continue</a>",
                "/navigation/step-two": "<h1>Second page</h1><a href='/navigation/finish'>Finish</a>",
                "/navigation/finish": "<h1>Navigation complete</h1>",
                "/table-extraction/": "<h1>Inventory</h1><table><thead><tr><th>Item</th>"
                "<th>Quantity</th></tr></thead><tbody><tr><td>Apples</td><td>3</td></tr>"
                "<tr><td>Pears</td><td>5</td></tr><tr><td>Oranges</td><td>2</td></tr></tbody>"
                "</table><form method='post' action='/table-extraction/submitted'>"
                "<label>Extracted rows<textarea name='rows'></textarea></label>"
                "<button>Save extraction</button></form>",
                "/login/": "<h1>Fixture login</h1><form method='post' action='/login/otp'>"
                "<label>Username<input name='username'></label>"
                "<label>Password<input name='password' type='password'></label>"
                "<button>Sign in</button></form>",
                "/download/": "<h1>Reports</h1><a href='/download/report.csv' download>Get report</a>",
            }
            if path == "/download/report.csv":
                self.reply(200, DOWNLOAD, task, (
                    ("Content-Type", "text/csv"),
                    ("Content-Disposition", 'attachment; filename="report.csv"'),
                ))
            elif path in pages:
                self.reply(200, page(pages[path]), task)
            else:
                self.reply(404, page("<h1>Not found</h1>"), task)

        def do_POST(self):
            path = urlsplit(self.path).path
            task = path.strip("/").split("/")[0]
            length = int(self.headers.get("Content-Length", "0"))
            if length > 8192:
                self.reply(413, b"Form too large", task)
                return
            form = {key: values[0] for key, values in
                    parse_qs(self.rfile.read(length).decode()).items()}
            with state.lock:
                state.submitted[path] = form
            if path == "/form-fill/submitted" and form == {"name": "Ada", "city": "Oslo"}:
                self.reply(200, page("<h1>Form submitted</h1>"), task)
            elif path == "/table-extraction/submitted":
                try:
                    correct = json.loads(form.get("rows", "null")) == TABLE
                except json.JSONDecodeError:
                    correct = False
                self.reply(200 if correct else 400, page("<h1>Extraction received</h1>"), task)
            elif path == "/login/otp" and form == {"username": "demo", "password": "fixture-only"}:
                self.reply(200, page(
                    "<h1>Two-factor verification</h1><form method='post' action='/login/verified'>"
                    "<label>Verification code<input name='code'></label><button>Verify code</button></form>"
                ), task, (("Set-Cookie", "fixture_login=pending; HttpOnly; SameSite=Strict; Path=/login/"),))
            elif (path == "/login/verified" and form == {"code": OTP}
                  and "fixture_login=pending" in self.headers.get("Cookie", "")):
                with state.lock:
                    state.login_complete = True
                self.reply(200, page("<h1>Login complete</h1>"), task)
            else:
                self.reply(400, page("<h1>Incorrect fixture submission</h1>"), task)

    return Handler


@contextmanager
def fixture_server():
    state = FixtureState()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(state))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}", state
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
        if thread.is_alive():
            raise RuntimeError("fixture server did not stop")

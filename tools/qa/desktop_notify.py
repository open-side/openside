#!/usr/bin/env python3
"""Desktop notifications in the patched openside browser (Settings -> Enable notifications).

Starts a hermetic opensided (scripted model, private HOME/XDG, loopback listener) and the patched
Chromium (headless, or headed inside the private sway of tools/qa/desktop-notify.sh) with the openside
MV3 extension, then drives the real Settings screen over CDP:

  repro   push-only page, no extension: PushManager.subscribe() rejects in the patched build and the
          page shows the push-service message (before the fix it showed "Request failed").
  smoke   extension loaded: the toggle pairs the extension, a scripted task runs, and the extension's
          chrome.notifications.create call log (and, headed, the notification daemon) shows the
          "Task finished" notification. A conversation the person is looking at stays silent.

Screenshots, result.json and logs land in --out. Never touches the user's daemon, profile or desktop.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import shutil
import signal
import subprocess
import tempfile
import time
from pathlib import Path

import websockets

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_PATCHED = ROOT.parent / "openside-browser-build/portablelinux/build/src/out/Default/chrome"
EXTENSION_ID = "jneddlbpgmlheplolfonafkbkbkjhcfj"


def run_text(*command: str) -> str:
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout


class Cdp:
    def __init__(self, ws) -> None:
        self.ws = ws
        self.next = 0
        self.events: list[dict] = []

    async def call(self, method: str, params: dict | None = None, session: str | None = None):
        self.next += 1
        ident = self.next
        message = {"id": ident, "method": method, "params": params or {}}
        if session:
            message["sessionId"] = session
        await self.ws.send(json.dumps(message))
        while True:
            reply = json.loads(await asyncio.wait_for(self.ws.recv(), 30))
            if reply.get("id") == ident:
                if "error" in reply:
                    raise RuntimeError(f"{method}: {reply['error']}")
                return reply["result"]
            self.events.append(reply)

    async def targets(self) -> list[dict]:
        return (await self.call("Target.getTargets"))["targetInfos"]

    async def attach(self, target: str) -> str:
        return (await self.call("Target.attachToTarget", {"targetId": target, "flatten": True}))["sessionId"]

    async def evaluate(self, session: str, expression: str):
        result = await self.call(
            "Runtime.evaluate",
            {"expression": expression, "awaitPromise": True, "returnByValue": True, "userGesture": True},
            session,
        )
        if "exceptionDetails" in result:
            raise RuntimeError(f"JS exception: {result['exceptionDetails']}")
        return result["result"].get("value")

    async def click(self, session: str, selector: str) -> None:
        box = await self.evaluate(
            session,
            f"(() => {{ const e = document.querySelector({json.dumps(selector)}); if (!e) return null;"
            "e.scrollIntoView({block:'center'}); const r = e.getBoundingClientRect();"
            "return [r.x + r.width / 2, r.y + r.height / 2]; })()",
        )
        if not box:
            raise RuntimeError(f"no element for {selector}")
        for kind in ("mousePressed", "mouseReleased"):
            await self.call(
                "Input.dispatchMouseEvent",
                {"type": kind, "x": box[0], "y": box[1], "button": "left", "clickCount": 1},
                session,
            )


async def wait_until(predicate, timeout: float = 15.0, step: float = 0.25):
    deadline = time.time() + timeout
    last = None
    while time.time() < deadline:
        try:
            last = await predicate()
        except Exception:  # target navigating or not attached yet
            last = None
        if last:
            return last
        await asyncio.sleep(step)
    return last


class Run:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.executable = Path(args.executable or os.environ.get("OPENSIDE_PATCHED_BROWSER") or DEFAULT_PATCHED)
        self.out = Path(args.out)
        self.out.mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(prefix="openside-qa-notify."))
        self.daemon: subprocess.Popen | None = None
        self.browser: subprocess.Popen | None = None
        self.helpers: list[subprocess.Popen] = []
        self.results: list[dict] = []
        self.origin = ""
        self.token = ""
        self.ws_url = ""
        self.profile = self.root / "profile"

    def record(self, check: str, ok: bool | None, detail: str) -> None:
        self.results.append({"check": check, "pass": ok, "detail": detail})
        verdict = "JUDGE" if ok is None else "PASS" if ok else "FAIL"
        print(f"[{self.args.scenario}] {verdict} {check}: {detail}", flush=True)

    # ---- daemon ----------------------------------------------------------------
    def env(self) -> dict[str, str]:
        r = self.root
        return {
            "HOME": str(r / "home"),
            "XDG_CONFIG_HOME": str(r / "config"),
            "XDG_DATA_HOME": str(r / "data"),
            "XDG_STATE_HOME": str(r / "state"),
            "XDG_CACHE_HOME": str(r / "cache"),
            "XDG_RUNTIME_DIR": str(r / "run"),
            "TMPDIR": str(r / "tmp"),
            "PATH": os.environ.get("PATH", "/usr/bin"),
            "OPENSIDE_AGENT_SCRIPT": str(r / "script.json"),
        }

    def start_daemon(self) -> None:
        r = self.root
        for name in ["home", "config/openside", "data", "state", "cache", "run", "tmp", "workspace"]:
            (r / name).mkdir(parents=True, exist_ok=True)
        (r / "run").chmod(0o700)
        dist = ROOT / "web/dist"
        if not (dist / "index.html").is_file():
            raise SystemExit("build web/dist first (pnpm -C web build)")
        python = ROOT / "agent/.venv/bin/python"
        (r / "config/openside/config.toml").write_text(
            f"[agent]\ncommand = {json.dumps([str(python), '-m', 'openside_agent'])}\n"
            '[models]\ndefault = "script"\n[policy]\nmode = "full"\n'
            f"[ui]\ndir = {json.dumps(str(dist))}\n"
        )
        (r / "script.json").write_text(json.dumps({
            "delay_ms": self.args.task_ms,
            "title": "Summarise the quarterly report",
            "responses": [{"parts": [{"type": "text", "text": "Done."}]}] * 4,
        }))
        data = r / "data/openside"
        self.daemon = subprocess.Popen(
            [str(self.args.daemon_bin), "--data-dir", str(data),
             "--socket", str(r / "run/opensided.sock"), "--listen", "127.0.0.1:0"],
            env=self.env(), cwd=r / "workspace", stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=open(r / "daemon.stderr", "wb"), start_new_session=True,
        )
        deadline = time.time() + 30
        while not (data / "opensided.port").is_file():
            if self.daemon.poll() is not None or time.time() > deadline:
                raise SystemExit(f"daemon failed: {(r / 'daemon.stderr').read_text()[-2000:]}")
            time.sleep(0.1)
        time.sleep(0.3)
        self.origin = f"http://127.0.0.1:{(data / 'opensided.port').read_text().strip()}"
        paired = self.unix("POST", "/v1/devices/pair", {"name": "notify-qa", "scopes": ["admin"]})
        self.token = paired["token"]
        self.device = paired.get("device_id", "")
        self.api("PUT", "/v1/onboarding", {"state": "skipped"})

    def unix(self, method: str, path: str, body: dict | None = None) -> dict:
        command = ["curl", "-sS", "--unix-socket", str(self.root / "run/opensided.sock"),
                   "-X", method, f"http://localhost{path}"]
        if body is not None:
            command += ["-H", "content-type: application/json", "-d", json.dumps(body)]
        out = run_text(*command)
        return json.loads(out) if out.strip() else {}

    def api(self, method: str, path: str, body: dict | None = None) -> dict:
        command = ["curl", "-sS", "-X", method, f"{self.origin}{path}", "-H", f"Authorization: Bearer {self.token}"]
        if body is not None:
            command += ["-H", "content-type: application/json", "-d", json.dumps(body)]
        out = run_text(*command)
        return json.loads(out) if out.strip() else {}

    # ---- browser ---------------------------------------------------------------
    def launch_browser(self) -> None:
        r = self.root
        (self.profile / "Default").mkdir(parents=True, exist_ok=True)
        env = self.env()
        env["XDG_RUNTIME_DIR"] = str(Path(tempfile.mkdtemp(prefix="osn", dir="/tmp")))
        self.runtime = Path(env["XDG_RUNTIME_DIR"])
        if self.args.headed:
            env["WAYLAND_DISPLAY"] = str(Path(os.environ["XDG_RUNTIME_DIR"]) / os.environ["WAYLAND_DISPLAY"])
            if "DBUS_SESSION_BUS_ADDRESS" in os.environ:
                env["DBUS_SESSION_BUS_ADDRESS"] = os.environ["DBUS_SESSION_BUS_ADDRESS"]
        args = [
            str(self.executable),
            f"--user-data-dir={self.profile}",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--no-default-browser-check",
            "--password-store=basic",
            "--disable-sync",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--window-size=1440,900",
        ]
        if self.args.headed:
            args += ["--ozone-platform=wayland", "--class=openside-qa-notify"]
        else:
            args += ["--headless=new"]
        if self.args.no_sandbox:
            args.append("--no-sandbox")
        if self.args.scenario == "smoke":
            args.append(f"--load-extension={self.args.extension}")
        args.append("about:blank")
        self.browser = subprocess.Popen(
            args, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=open(r / "browser.stderr", "wb"), start_new_session=True,
        )
        # A headed start under software rendering on a loaded machine can take well over 30 s.
        deadline = time.time() + 180
        while not (self.profile / "DevToolsActivePort").is_file():
            if self.browser.poll() is not None or time.time() > deadline:
                raise SystemExit(f"browser failed (exit {self.browser.poll()}): {(r / 'browser.stderr').read_text()[-2000:]}")
            time.sleep(0.1)
        port, path = (self.profile / "DevToolsActivePort").read_text().split()[:2]
        self.ws_url = f"ws://127.0.0.1:{port}{path}"

    def stop(self) -> None:
        for process in [self.browser, self.daemon, *self.helpers]:
            if process and process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        marker = str(self.root).encode()
        for proc in Path("/proc").iterdir():
            if not proc.name.isdigit() or int(proc.name) == os.getpid():
                continue
            try:
                if marker in (proc / "cmdline").read_bytes() or marker in (proc / "environ").read_bytes():
                    os.kill(int(proc.name), signal.SIGKILL)
            except OSError:
                pass
        time.sleep(0.5)
        for log in ("browser.stderr", "daemon.stderr"):
            if (self.root / log).is_file():
                shutil.copy(self.root / log, self.out / log)
        shutil.rmtree(self.root, ignore_errors=True)
        shutil.rmtree(getattr(self, "runtime", self.root), ignore_errors=True)

    async def shot(self, cdp: "Cdp", session: str, name: str) -> None:
        """Page screenshot over CDP, plus the whole compositor frame when headed."""
        import base64
        data = (await cdp.call("Page.captureScreenshot", {"format": "png"}, session))["data"]
        (self.out / f"{name}.png").write_bytes(base64.b64decode(data))
        if self.args.headed:
            subprocess.run(["grim", str(self.out / f"{name}-desktop.png")], check=True)


async def open_ui(run: Run, cdp: Cdp, path: str = "/settings") -> str:
    """A tab signed in the way tools/openside-browser does it: the daemon's local-access launch URL."""
    access = run.unix("GET", "/v1/remote/local-access")
    target = (await cdp.call("Target.createTarget", {"url": "about:blank"}))["targetId"]
    session = await cdp.attach(target)
    await cdp.call("Page.enable", {}, session)
    await cdp.call("Page.navigate", {"url": access["url"]}, session)
    await wait_until(lambda: cdp.evaluate(session, "!!localStorage.getItem('openside.token') && document.readyState === 'complete'"), 20)
    await cdp.call("Page.navigate", {"url": f"{run.origin}{path}"}, session)
    await wait_until(lambda: cdp.evaluate(session, "document.readyState === 'complete'"), 20)
    return session


async def push_only_repro(run: Run, cdp: Cdp) -> None:
    """What the owner sees in the patched build: subscribe() rejects, and the old Settings code turned that into "Request failed"."""
    await cdp.call("Browser.grantPermissions", {"permissions": ["notifications"], "origin": run.origin})
    session = await open_ui(run, cdp)
    raw = await cdp.evaluate(
        session,
        """(async () => {
          const reg = await navigator.serviceWorker.register('/sw.js').then(() => navigator.serviceWorker.ready);
          const key = (await (await fetch('/v1/push/vapid', {headers: {Authorization: 'Bearer ' + localStorage.getItem('openside.token')}})).json()).public_key;
          const bytes = Uint8Array.from(atob(key.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));
          try { await reg.pushManager.subscribe({userVisibleOnly: true, applicationServerKey: bytes}); return {subscribed: true}; }
          catch (e) {
            // The pre-fix branch of SettingsPage: `cause instanceof Error && !("code" in cause) ? cause.message : errorMessage(cause)`,
            // where errorMessage() answers "Request failed" for anything that is not an ApiError.
            const shown = e instanceof Error && !("code" in e) ? e.message : "Request failed";
            return {name: e.name, message: e.message, code: e.code, instanceofError: e instanceof Error, hasCode: "code" in e, oldUiShowed: shown};
          }
        })()""",
    )
    run.record("PushManager.subscribe() in the patched build rejects", raw.get("name") == "AbortError", json.dumps(raw))
    run.record("pre-fix Settings text for that rejection was the generic one", raw.get("oldUiShowed") == "Request failed", str(raw.get("oldUiShowed")))
    await run.shot(cdp, session, "repro-settings")


HOOK = """
globalThis.__notified = [];
const original = chrome.notifications.create.bind(chrome.notifications);
chrome.notifications.create = (id, options, callback) => {
  globalThis.__notified.push({ id, title: options.title, message: options.message, at: Date.now() });
  return original(id, options, callback);
};
true
"""
EXT_CALL = ("new Promise(r => chrome.runtime.sendMessage(%s, %%s, r))" % json.dumps(EXTENSION_ID))


def focus_browser(run: Run) -> None:
    """The private sway has no pointer or keyboard user; give the browser window focus the way a person clicking it would."""
    if run.args.headed:
        subprocess.run(["swaymsg", "[app_id=openside-qa-notify] focus"], check=False, capture_output=True)


async def settings_text(cdp: Cdp, session: str) -> str:
    return str(await cdp.evaluate(session, "document.querySelector('section[aria-label=Notifications]')?.innerText ?? ''"))


async def start_task(run: Run, text: str) -> str:
    created = run.api("POST", "/v1/sessions", {"title": None, "model": None, "mode": None, "workspace": None})
    run.api("POST", f"/v1/sessions/{created['id']}/messages", {"message_id": f"m-{created['id']}", "text": text, "mode": "now"})
    return created["id"]


async def smoke(run: Run, cdp: Cdp) -> None:
    await cdp.call("Browser.grantPermissions", {"permissions": ["notifications"], "origin": run.origin})
    worker = await wait_until(lambda: find_worker(cdp), 30)
    run.record("extension service worker is running", bool(worker), str(worker and worker["url"]))
    if not worker:
        return
    sw = await cdp.attach(worker["targetId"])
    await cdp.call("Runtime.enable", {}, sw)
    await cdp.evaluate(sw, HOOK)

    ui = await open_ui(run, cdp)
    focus_browser(run)
    await wait_until(lambda: cdp.evaluate(ui, "!!document.querySelector('section[aria-label=Notifications] button')"), 20)
    await run.shot(cdp, ui, "1-settings-before")
    await cdp.click(ui, "section[aria-label=Notifications] button")
    done = await wait_until(lambda: check_text(cdp, ui, "Enabled (desktop)"), 30)
    text = await settings_text(cdp, ui)
    run.record("Settings shows desktop notifications on after Enable", bool(done), text.replace("\n", " | "))
    await run.shot(cdp, ui, "2-settings-desktop-enabled")
    run.record("no generic error on Settings", "Request failed" not in text, "")
    status = await cdp.evaluate(ui, EXT_CALL % json.dumps({"type": "openside.desktop.status"}))
    run.record("extension reports paired + enabled", bool(status and status.get("paired") and status.get("enabled")), json.dumps(status))
    devices = run.api("GET", "/v1/devices")
    names = [d.get("name") for d in devices.get("devices", [])]
    run.record("extension holds its own device token, not the admin one", "openside extension" in names, str(names))

    # A conversation the person is not looking at: the tab stays on Settings.
    other = await start_task(run, "Summarise the quarterly report")
    await asyncio.sleep(2)
    await cdp.evaluate(ui, EXT_CALL % json.dumps({"type": "openside.desktop.enable"}))
    deadline = time.time() + run.args.task_ms / 1000 + 60
    fired = None
    while time.time() < deadline and not fired:
        calls = await cdp.evaluate(sw, "globalThis.__notified")
        fired = next((c for c in calls if c["id"] == f"settled:{other}"), None)
        await asyncio.sleep(1)
    calls = await cdp.evaluate(sw, "globalThis.__notified")
    (run.out / "notifications-create-calls.json").write_text(json.dumps(calls, indent=2))
    run.record("chrome.notifications.create fired for the finished task", bool(fired), json.dumps(fired))
    if run.args.headed:
        await asyncio.sleep(1.5)
        run.record("notification rendered by the desktop notification daemon", None, "see 3-notification-desktop.png and mako-history.txt")
        subprocess.run(["grim", str(run.out / "3-notification-desktop.png")], check=False)
    await run.shot(cdp, ui, "3-settings-after-task")

    # The conversation the person is looking at stays silent.
    watched = await start_task(run, "Draft the reply")
    await cdp.call("Page.navigate", {"url": f"{run.origin}/sessions/{watched}"}, ui)
    await cdp.call("Page.bringToFront", {}, ui)
    focus_browser(run)
    await asyncio.sleep(2)
    await cdp.evaluate(ui, EXT_CALL % json.dumps({"type": "openside.desktop.enable"}))
    await asyncio.sleep(run.args.task_ms / 1000 + 25)
    calls = await cdp.evaluate(sw, "globalThis.__notified")
    quiet = not any(c["id"] == f"settled:{watched}" for c in calls)
    focus = await cdp.evaluate(sw, "chrome.windows.getLastFocused({populate: true}).then(w => ({focused: w.focused, urls: (w.tabs || []).filter(t => t.active).map(t => t.url)}))")
    run.record("focused conversation stays quiet", quiet if focus.get("focused") else None, json.dumps(focus))

    await cdp.call("Page.navigate", {"url": f"{run.origin}/settings"}, ui)
    await wait_until(lambda: cdp.evaluate(ui, "!!document.querySelector('section[aria-label=Notifications] button')"), 20)
    await cdp.click(ui, "section[aria-label=Notifications] button")
    off = await wait_until(lambda: check_text(cdp, ui, "Off"), 20)
    run.record("Turn off notifications switches the extension off and revokes its token", bool(off) and "openside extension" not in [d.get("name") for d in run.api("GET", "/v1/devices").get("devices", [])], await settings_text(cdp, ui))


async def check_text(cdp: Cdp, session: str, want: str) -> bool:
    return want in await settings_text(cdp, session)


async def find_worker(cdp: Cdp):
    for target in await cdp.targets():
        if target["type"] == "service_worker" and EXTENSION_ID in target["url"] and target["url"].endswith("background.js"):
            return target
    return None


async def no_extension(run: Run, cdp: Cdp) -> None:
    """The patched build without the extension: the toggle says what is wrong and what to install."""
    await cdp.call("Browser.grantPermissions", {"permissions": ["notifications"], "origin": run.origin})
    ui = await open_ui(run, cdp)
    await wait_until(lambda: cdp.evaluate(ui, "!!document.querySelector('section[aria-label=Notifications] button')"), 20)
    await cdp.click(ui, "section[aria-label=Notifications] button")
    alert = await wait_until(lambda: cdp.evaluate(ui, "document.querySelector('section[aria-label=Notifications] [role=alert]')?.textContent"), 30)
    await run.shot(cdp, ui, "noext-error")
    run.record("without the extension the error names the push service and the extension", bool(alert) and "push service is not available" in alert and "Install the openside extension" in alert and "Request failed" not in alert, str(alert))


async def main_async(run: Run) -> None:
    async with websockets.connect(run.ws_url, max_size=None) as ws:
        cdp = Cdp(ws)
        await {"repro": push_only_repro, "noext": no_extension, "smoke": smoke}[run.args.scenario](run, cdp)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("scenario", choices=["repro", "noext", "smoke"])
    parser.add_argument("--out", default="/tmp/openside-qa/round2/NotifyFix")
    parser.add_argument("--executable")
    parser.add_argument("--daemon-bin", default=str(ROOT / "target/debug/opensided"))
    parser.add_argument("--extension")
    parser.add_argument("--headed", action="store_true")
    parser.add_argument("--no-sandbox", action="store_true")
    parser.add_argument("--task-ms", type=int, default=0)
    args = parser.parse_args()
    args.extension = args.extension or str(Path(args.out) / "extension")
    run = Run(args)
    try:
        run.start_daemon()
        run.launch_browser()
        asyncio.run(main_async(run))
    finally:
        run.stop()
        (run.out / f"result-{args.scenario}.json").write_text(json.dumps(run.results, indent=2))
    return 0 if all(r["pass"] is not False for r in run.results) else 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Headed browser UX QA (UX-01/02/04, P8/P9/P11 entry points, MV3 Ask fallback).

Starts a hermetic opensided (scripted model, private HOME/XDG, loopback listener),
launches the patched or stock Chromium headed on Wayland with the openside MV3
extension, drives it with real compositor key events plus CDP, captures screenshots
and checks that the active window never changes because of the agent.

Runs inside a private headless sway that tools/qa/headed-ux.sh starts in a rootless,
network-less container: keys via wtype, clicks via a wlr virtual pointer
(tools/qa/wl_pointer.py), screenshots via grim, focus via sway IPC.

Run through tools/qa/headed-ux.sh. Screenshots and result.json land in --out/sway/<browser>.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import secrets
import shutil
import signal
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import websockets

ROOT = Path(__file__).resolve().parents[2]
DEFAULT_PATCHED = ROOT.parent / "openside-browser-build/portablelinux/build/src/out/Default/chrome"
STOCK = Path("/usr/bin/chromium")
PAIRED = "Paired for this browser session."
ANSWER = "Scripted openside answer."


def run_text(*command: str) -> str:
    return subprocess.run(command, check=True, capture_output=True, text=True).stdout


class Sway:
    name = "sway"

    def __init__(self, cls: str) -> None:
        self.cls = cls

    def tree(self) -> dict:
        return json.loads(run_text("swaymsg", "-t", "get_tree", "-r"))

    def nodes(self, node: dict | None = None):
        node = node or self.tree()
        yield node
        for child in node.get("nodes", []) + node.get("floating_nodes", []):
            yield from self.nodes(child)

    def active(self) -> str:
        for node in self.nodes():
            if node.get("focused"):
                return str(node["id"])
        return ""

    def describe(self) -> str:
        for node in self.nodes():
            if node.get("focused"):
                return f"{node.get('app_id')} | {node.get('name')}"
        return ""

    def prepare(self) -> None:
        # Headless sway has no pointer device; a wlr virtual pointer stands in for the mouse.
        self.fifo = Path(tempfile.mkdtemp(prefix="osqp", dir="/tmp")) / "pointer"
        os.mkfifo(self.fifo)
        output = json.loads(run_text("swaymsg", "-t", "get_outputs", "-r"))[0]["rect"]
        self.pointer = subprocess.Popen(
            ["/usr/bin/python3", str(ROOT / "tools/qa/wl_pointer.py"), str(self.fifo),
             str(output["width"]), str(output["height"])],
            stdout=subprocess.PIPE, text=True, start_new_session=True)
        if self.pointer.stdout.readline().strip() != "ready":
            raise RuntimeError("virtual pointer failed to start")

    def cleanup(self) -> None:
        self.pointer.kill()
        self.pointer.wait()
        shutil.rmtree(self.fifo.parent, ignore_errors=True)

    def click(self, x: int, y: int) -> None:
        with open(self.fifo, "w") as fifo:
            fifo.write(f"click {x} {y}\n")
        time.sleep(0.4)

    def window(self) -> dict | None:
        for node in self.nodes():
            if node.get("app_id") == self.cls:
                return {"id": str(node["id"]), "where": "headless sway"}
        return None

    MODS = {"CTRL": "ctrl", "ALT": "alt", "SHIFT": "shift"}

    def key(self, key: str, mods: str = "") -> None:
        pressed = [self.MODS[m] for m in mods.split()]
        # Each wtype run uploads a fresh virtual keymap; Chromium drops events sent before it settles.
        command = ["wtype", "-s", "150"]
        for mod in pressed:
            command += ["-M", mod]
        command += ["-k", key]
        for mod in reversed(pressed):
            command += ["-m", mod]
        subprocess.run(command, check=True)
        time.sleep(0.08)

    def type(self, text: str) -> None:
        subprocess.run(["wtype", "-s", "150", "-d", "20", text], check=True)

    def shot(self, path: Path) -> None:
        subprocess.run(["grim", str(path)], check=True)


class FocusWatch:
    """Samples the compositor's active window for the whole browser run."""

    def __init__(self, desktop) -> None:
        self.desktop = desktop
        self.samples: list[str] = []
        # Every active-window change with the harness step that was running, for attribution.
        self.changes: list[dict] = []
        self.step = "start"
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        last = None
        while not self._stop.is_set():
            try:
                active = self.desktop.active()
            except Exception:  # a busy compositor; the next sample decides
                active = None
            if active is not None:
                self.samples.append(active)
                if active != last:
                    self.changes.append({"t": round(time.time(), 2), "active": active, "step": self.step,
                                         "window": self.desktop.describe()})
                    last = active
            self._stop.wait(0.1)

    def start(self) -> None:
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        if self._thread.is_alive():
            self._thread.join()


class Cdp:
    def __init__(self, ws) -> None:
        self.ws = ws
        self.next = 0

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

    async def targets(self) -> list[dict]:
        return (await self.call("Target.getTargets"))["targetInfos"]

    async def pages(self) -> list[dict]:
        return [t for t in await self.targets() if t["type"] == "page"]

    async def attach(self, target: str) -> str:
        return (await self.call("Target.attachToTarget", {"targetId": target, "flatten": True}))["sessionId"]

    async def evaluate(self, session: str, expression: str):
        result = await self.call(
            "Runtime.evaluate",
            {"expression": expression, "awaitPromise": True, "returnByValue": True},
            session,
        )
        if "exceptionDetails" in result:
            raise RuntimeError(f"JS exception: {result['exceptionDetails']}")
        return result["result"].get("value")

    async def url_page(self, want: str):
        for page in await self.pages():
            if page["url"] == want:
                return page
        return None


async def wait_until(predicate, timeout: float = 15.0, step: float = 0.2):
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
        self.kind = args.browser
        if self.kind == "patched":
            self.executable = Path(args.executable or os.environ.get("OPENSIDE_PATCHED_BROWSER") or DEFAULT_PATCHED)
        else:
            self.executable = Path(args.executable or STOCK)
        self.cls = f"openside-qa-uxnative-{self.kind}"
        self.desktop = Sway(self.cls)
        self.out = Path(args.out) / "sway" / self.kind
        self.out.mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(prefix=f"openside-qa-ux-{self.kind}."))
        self.daemon: subprocess.Popen | None = None
        self.browser: subprocess.Popen | None = None
        self.results: list[dict] = []
        self.origin = ""
        self.token = ""
        self.ws_url = ""
        self.runtime: Path | None = None
        self.watch: FocusWatch | None = None

    def record(self, check: str, ok: bool | None, detail: str, shot: str | None = None) -> None:
        """ok=None means the screenshot is the evidence and needs a human/model verdict."""
        entry = {"check": check, "pass": ok if ok is None else bool(ok), "detail": detail}
        if shot:
            entry["screenshot"] = shot
        self.results.append(entry)
        if self.watch:
            self.watch.step = f"after check: {check}"
        verdict = "JUDGE" if ok is None else "PASS" if ok else "FAIL"
        print(f"[{self.desktop.name}/{self.kind}] {verdict} {check}: {detail}", flush=True)

    def shot(self, name: str) -> str:
        path = self.out / f"{name}.png"
        self.desktop.shot(path)
        if self.watch:
            self.watch.step = f"after shot: {name}"
        return str(path)

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
        (r / "script.json").write_text(json.dumps(
            {"responses": [{"parts": [{"type": "text", "text": ANSWER}]}] * 8}))
        data = r / "data/openside"
        self.daemon = subprocess.Popen(
            [str(ROOT / "target/debug/opensided"), "--data-dir", str(data),
             "--socket", str(r / "run/opensided.sock"), "--listen", "127.0.0.1:0"],
            env=self.env(), cwd=r / "workspace", stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL, stderr=open(r / "daemon.stderr", "wb"),
            start_new_session=True,
        )
        deadline = time.time() + 30
        while not (data / "opensided.port").is_file():
            if self.daemon.poll() is not None or time.time() > deadline:
                raise SystemExit(f"daemon failed: {(r / 'daemon.stderr').read_text()[-2000:]}")
            time.sleep(0.1)
        time.sleep(0.3)
        self.origin = f"http://127.0.0.1:{(data / 'opensided.port').read_text().strip()}"
        self.token = self.unix("POST", "/v1/devices/pair",
                               {"name": "headed-ux-qa", "scopes": ["read", "steer", "approve"]})["token"]

    def unix(self, method: str, path: str, body: dict | None = None) -> dict:
        command = ["curl", "-sS", "--unix-socket", str(self.root / "run/opensided.sock"),
                   "-X", method, f"http://localhost{path}"]
        if body is not None:
            command += ["-H", "content-type: application/json", "-d", json.dumps(body)]
        out = run_text(*command)
        return json.loads(out) if out.strip() else {}

    def sessions(self) -> list[dict]:
        return self.unix("GET", "/v1/sessions").get("sessions", [])

    # ---- browser ---------------------------------------------------------------
    def launch_browser(self) -> None:
        r = self.root
        profile = r / "profile"
        # Pin the stock "Show bookmarks in side panel" toolbar action so UX-04 is one click away.
        (profile / "Default").mkdir(parents=True, exist_ok=True)
        (profile / "Default/Preferences").write_text(
            json.dumps({"toolbar": {"pinned_actions": ["kActionSidePanelShowBookmarks"]}}))
        # The gateway socket path must fit sockaddr_un (108 bytes), so keep this directory short.
        runtime = Path(tempfile.mkdtemp(prefix="osq", dir="/tmp"))
        self.runtime = runtime
        env = self.env()
        # The gateway requires its own private XDG_RUNTIME_DIR; Wayland is reached by absolute path.
        env["XDG_RUNTIME_DIR"] = str(runtime)
        env["WAYLAND_DISPLAY"] = str(Path(os.environ["XDG_RUNTIME_DIR"]) / os.environ["WAYLAND_DISPLAY"])
        if "DBUS_SESSION_BUS_ADDRESS" in os.environ:
            env["DBUS_SESSION_BUS_ADDRESS"] = os.environ["DBUS_SESSION_BUS_ADDRESS"]
        args = [
            str(self.executable),
            f"--user-data-dir={profile}",
            f"--class={self.cls}",
            "--ozone-platform=wayland",
            "--remote-debugging-port=0",
            "--remote-debugging-address=127.0.0.1",
            "--no-first-run",
            "--no-default-browser-check",
            "--password-store=basic",
            "--disable-sync",
            "--disable-background-networking",
            "--disable-component-update",
            "--disable-default-apps",
            "--disable-features=AimEnabled,PreconnectToSearch,Translate,MediaRouter",
            "--window-size=1600,1000",
            f"--load-extension={self.args.extension}",
        ]
        if self.args.no_sandbox:
            # Rootless containers cannot nest Chromium's namespace sandbox.
            args.append("--no-sandbox")
        if self.kind == "patched":
            # Exactly the documented P8 switch plus the daemon's gateway/agent launch switches.
            args += [
                f"--openside-daemon-ui-url={self.origin}/",
                f"--openside-gateway-instance-id={secrets.token_hex(16)}",
                "--openside-agent-session-id=qa-headed-ux",
            ]
        args.append("about:blank")
        self.browser = subprocess.Popen(
            args, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
            stderr=open(r / "browser.stderr", "wb"), start_new_session=True,
        )
        deadline = time.time() + 30
        while not (profile / "DevToolsActivePort").is_file():
            if self.browser.poll() is not None or time.time() > deadline:
                raise SystemExit(f"browser failed: {(r / 'browser.stderr').read_text()[-2000:]}")
            time.sleep(0.1)
        port, path = (profile / "DevToolsActivePort").read_text().split()[:2]
        self.ws_url = f"ws://127.0.0.1:{port}{path}"

    def stop(self) -> None:
        for process in (self.browser, self.daemon):
            if process and process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
        # Chromium and sidecar children can leave their group; match only this run's private root.
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
        try:
            self.desktop.cleanup()
        except Exception as error:
            print(f"desktop cleanup: {error}")
        for log in ("browser.stderr", "daemon.stderr"):
            if (self.root / log).is_file():
                shutil.copy(self.root / log, self.out / log)
        shutil.rmtree(self.root, ignore_errors=True)
        if self.runtime:
            shutil.rmtree(self.runtime, ignore_errors=True)


async def checks(run: Run, watch: FocusWatch) -> str:
    async with websockets.connect(run.ws_url, max_size=None) as ws:
        cdp = Cdp(ws)
        window = await wait_until(lambda: asyncio.sleep(0, run.desktop.window()), 20)
        run.record("browser window appears", bool(window), f"window={window}")
        await asyncio.sleep(2.5)
        baseline = run.desktop.active()
        watch.start()

        # P9 / UX-01: a fresh profile shows vertical tabs (default, no explicit preference).
        prefs_path = run.root / "profile/Default/Preferences"
        prefs = json.loads(prefs_path.read_text() or "{}") if prefs_path.is_file() else {}
        explicit = prefs.get("vertical_tabs", {}).get("enabled")
        shot = run.shot("01-initial-window")
        run.record("UX-01 vertical tabs default", None, f"explicit pref={explicit}", shot)

        # P8: a user new tab (keyboard, not CDP) opens the daemon web UI when patched.
        before = {t["targetId"] for t in await cdp.pages()}
        run.desktop.key("t", "CTRL")
        new = await wait_until(lambda: _new_page(cdp, before), 10)
        expected = run.origin + "/"
        page = await cdp.attach(new["targetId"]) if new else None
        # The target list reports the NTP's virtual URL (chrome://newtab/); the document is what loaded.
        href = await wait_until(
            lambda: cdp.evaluate(page, f"location.href === {json.dumps(expected)} && location.href"),
            10) if page else None
        if not href and page:
            href = await cdp.evaluate(page, "location.href")
        await asyncio.sleep(2)
        shot = run.shot("02-new-tab")
        ok = href == expected if run.kind == "patched" else bool(href) and not href.startswith(run.origin)
        run.record("P8 new tab page", ok,
                   f"Ctrl+T virtual={new and new['url']} document={href} daemon={expected}", shot)
        if run.kind == "patched" and href == expected:
            page = await cdp.attach(new["targetId"])
            token_screen = await wait_until(lambda: cdp.evaluate(page, "!!document.querySelector('.token-entry')"), 10)
            run.record("P8 NTP keeps normal web UI pairing", bool(token_screen),
                       "unpaired daemon UI shows its token screen", shot)
            await cdp.evaluate(page, f"localStorage.setItem('openside.token', {json.dumps(run.token)}); location.reload(); true")
            await asyncio.sleep(2.5)
            run.record("P8 NTP web UI after pairing", None, "home screen", run.shot("02b-new-tab-paired"))

        # Diagnostic: the profile's keyword list as a user sees it in settings.
        run.desktop.key("l", "CTRL")
        run.desktop.type("chrome://settings/searchEngines")
        run.desktop.key("Return")
        await asyncio.sleep(3)
        run.record("search engine keywords (settings)", None, "ask/routines listed?",
                   run.shot("02c-settings-search-engines"))

        # P8/P11: omnibox keywords open the web UI and never auto-submit.
        for keyword, path in (("ask", "/"), ("routines", "/routines")):
            sessions_before = len(run.sessions())
            run.desktop.key("l", "CTRL")
            run.desktop.type(f"{keyword} hello qa")
            await asyncio.sleep(0.8)
            run.shot(f"03-omnibox-{keyword}-typed")
            run.desktop.key("Return")
            want = f"{run.origin}{path}?q=hello+qa"
            found = await wait_until(lambda: cdp.url_page(want), 10)
            await asyncio.sleep(2.5)
            shot = run.shot(f"04-omnibox-{keyword}-result")
            urls = [t["url"] for t in await cdp.pages()]
            sessions_after = len(run.sessions())
            if run.kind == "patched":
                run.record(f"P8/P11 omnibox '{keyword}' opens {path}?q=… without auto-submit",
                           bool(found) and sessions_after == sessions_before,
                           f"url={found and found['url']} sessions {sessions_before}->{sessions_after}", shot)
            else:
                run.record(f"stock: no '{keyword}' keyword", not found, f"pages={urls}", shot)

        # UX-04: bookmarks side panel. Click the pinned "Show bookmarks in side panel" toolbar button
        # (seeded in the profile); headless sway does not open the app menu from Alt+F.
        run.shot("06a-before-bookmarks-button")
        run.desktop.click(*(run.args.bookmarks_button or ((1418, 49) if run.kind == "patched" else (1485, 90))))

        async def bookmarks_panel():
            for t in await cdp.targets():
                if t["url"].startswith("chrome://bookmarks-side-panel"):
                    return t["url"]
            return None

        panel_url = await wait_until(bookmarks_panel, 6)
        await asyncio.sleep(1.5)
        run.record("UX-04 bookmarks side panel", bool(panel_url), f"side panel WebUI target={panel_url}",
                   run.shot("06-bookmarks-side-panel"))

        # UX-02: split view with the native shortcut (Alt+Shift+N), then pick the other tab.
        # Return keyboard focus from the side panel to the page first (omnibox, then Escape).
        run.desktop.key("l", "CTRL")
        run.desktop.key("Escape")
        run.desktop.key("Escape")
        run.desktop.key("n", "ALT SHIFT")
        await asyncio.sleep(2.5)
        run.shot("05a-split-view-picker")
        run.desktop.key("Return")
        await asyncio.sleep(2.0)
        split = await split_tabs(cdp)
        run.record("UX-02 split view (Alt+Shift+N)", len(split) >= 2,
                   f"tabs in a split (chrome.tabs splitViewId)={split}", run.shot("05-split-view"))

        await extension_ask(run, cdp)

        # Agent Tabs (P3/P4, patched): an agent CDP target joins "Agent Tabs" in the user's window.
        focus_before = run.desktop.active()
        created = await cdp.call("Target.createTarget", {"url": f"{run.origin}/", "background": True})
        await asyncio.sleep(3)
        shot = run.shot("08-agent-tab")
        run.record("agent tab opened by CDP", None,
                   f"target={created.get('targetId')} focus unchanged={run.desktop.active() == focus_before}", shot)
        window_target = await cdp.call("Target.createTarget",
                                       {"url": f"{run.origin}/", "newWindow": True, "background": True})
        await asyncio.sleep(2.5)
        after = run.desktop.active()
        run.record("agent window does not take focus", after == focus_before, f"before={focus_before} after={after}")
        # P5: agent pages must lay out at 1440x900 (gateway screenshots refuse other viewports).
        for label, target in (("shared-window background agent tab", created.get("targetId")),
                              ("dedicated agent window", window_target.get("targetId"))):
            session = await cdp.attach(target)
            size = await cdp.evaluate(session, "[innerWidth, innerHeight, devicePixelRatio]")
            bounds = (await cdp.call("Browser.getWindowForTarget", {"targetId": target}))["bounds"]
            # Stock Chromium has no P5 patch; its numbers are a reference only.
            run.record(f"P5 agent viewport 1440x900: {label}",
                       size[:2] == [1440, 900] if run.kind == "patched" else None,
                       f"inner={size[0]}x{size[1]} dpr={size[2]} window={bounds}")
        return baseline


async def split_tabs(cdp: Cdp) -> list[str]:
    """Titles of tabs Chromium reports inside a split view (via the extension's tabs API)."""
    for t in await cdp.targets():
        if t["type"] == "service_worker" and t["url"].endswith("/background.js"):
            worker = await cdp.attach(t["targetId"])
            return await cdp.evaluate(worker, (
                "chrome.tabs.query({}).then(tabs => tabs.filter(t => t.splitViewId !== undefined"
                " && t.splitViewId !== chrome.tabs.SPLIT_VIEW_ID_NONE).map(t => t.title || t.url))"))
    return []


async def _new_page(cdp: Cdp, before: set[str]):
    for target in await cdp.pages():
        if target["targetId"] not in before:
            return target
    return None


async def extension_ask(run: Run, cdp: Cdp) -> None:
    async def worker():
        for t in await cdp.targets():
            if t["type"] == "service_worker" and t["url"].endswith("/background.js"):
                return t
        return None

    found = await wait_until(worker, 15)
    if not found:
        run.record("MV3 extension loaded", False, "no service worker")
        return
    ext = found["url"].removesuffix("background.js")
    run.record("MV3 extension loaded", True, ext)
    service = await cdp.attach(found["targetId"])
    await cdp.evaluate(service, "chrome.runtime.openOptionsPage().then(() => true)")
    options = await wait_until(lambda: cdp.url_page(f"{ext}options.html"), 10)
    page = await cdp.attach(options["targetId"])
    await wait_until(lambda: cdp.evaluate(page, "!!document.querySelector('#url')"), 10)
    await cdp.evaluate(page, (
        f"document.querySelector('#url').value = {json.dumps(run.origin)};"
        f"document.querySelector('#token').value = {json.dumps(run.token)};"
        "document.querySelector('#token').focus(); true"))
    # A real Enter keypress submits the form inside a user gesture, as permissions.request needs.
    run.desktop.key("Return")
    await asyncio.sleep(1.5)
    run.shot("07a-host-permission-prompt")

    async def paired():
        status = await cdp.evaluate(page, "document.querySelector('#status').textContent")
        return status if status == PAIRED else None

    status = await paired()
    if not status:
        # Chromium's host-permission dialog focuses "Deny"; Tab moves to "Allow".
        run.desktop.key("Tab")
        run.desktop.key("Return")
        status = await wait_until(paired, 8)
    status = status or await cdp.evaluate(page, "document.querySelector('#status').textContent")
    run.record("extension pairing via options page", status == PAIRED, f"status={status!r}",
               run.shot("07b-options-paired"))
    # The extension swallows sidePanel.open() failures; keep the reason for the report.
    await cdp.evaluate(service, (
        "(() => { const open = chrome.sidePanel.open.bind(chrome.sidePanel);"
        " chrome.sidePanel.open = (...a) => open(...a).then(v => { globalThis.__qaOpen = 'ok'; return v; },"
        " e => { globalThis.__qaOpen = String(e && e.message || e); throw e; }); return true; })()"))
    sessions_before = len(run.sessions())
    run.desktop.key("l", "CTRL")
    run.desktop.type("os hello side panel")
    await asyncio.sleep(0.8)
    run.shot("07c-omnibox-os-typed")
    run.desktop.key("Return")

    async def panel():
        for t in await cdp.targets():
            if t["url"].startswith(f"{ext}panel.html"):
                return t
        return None

    side = await wait_until(panel, 10)
    await asyncio.sleep(2)
    sessions_after = len(run.sessions())
    open_result = await cdp.evaluate(service, "globalThis.__qaOpen ?? 'not called'")
    run.record("Ask side panel opens from omnibox 'os' and starts one session",
               bool(side) and sessions_after == sessions_before + 1,
               f"panel={bool(side)} sidePanel.open={open_result!r} sessions {sessions_before}->{sessions_after}",
               run.shot("07d-ask-side-panel"))
    if side:
        view = await cdp.attach(side["targetId"])

        async def answer():
            text = await cdp.evaluate(view, "document.body.innerText")
            return text if ANSWER in text else None

        text = await wait_until(answer, 20)
        run.record("Ask side panel renders the streamed answer", bool(text),
                   f"contains scripted answer={bool(text)}", run.shot("07e-ask-side-panel-answer"))


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--browser", choices=["patched", "stock"], default="patched")
    parser.add_argument("--executable")
    parser.add_argument("--extension", default=str(ROOT / "extension/dist"))
    parser.add_argument("--out", default="/tmp/openside-qa/uxnative")
    # Pinned toolbar actions sit just left of the avatar/menu (anchored to the right edge). Default:
    # patched vertical-tabs toolbar (1418,49); stock horizontal tab strip pushes it to (1485,90).
    parser.add_argument("--bookmarks-button", type=int, nargs=2, metavar=("X", "Y"))
    parser.add_argument("--no-sandbox", action="store_true")
    args = parser.parse_args()
    if not os.environ.get("WAYLAND_DISPLAY"):
        raise SystemExit("needs WAYLAND_DISPLAY")
    run = Run(args)
    watch = FocusWatch(run.desktop)
    run.watch = watch
    try:
        run.start_daemon()
        run.desktop.prepare()
        run.launch_browser()
        reference = asyncio.run(checks(run, watch))
        watch.stop()
        foreign = sorted({s for s in watch.samples if s and s != reference})
        run.record("active window never changed by the agent", not foreign,
                   f"{len(watch.samples)} samples; reference={reference}; other active windows={foreign}")
    finally:
        watch.stop()
        run.stop()
        (run.out / "result.json").write_text(json.dumps(run.results, indent=2))
        (run.out / "focus-changes.json").write_text(json.dumps(watch.changes, indent=2))
    return 1 if any(r["pass"] is False for r in run.results) else 0


if __name__ == "__main__":
    raise SystemExit(main())

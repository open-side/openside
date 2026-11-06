"""Hermetic desktop launcher checks; no user daemon or browser is started."""
import html.parser
import json
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import stat
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
SECRET = "a" * 64
TOKEN = "b" * 64
URL = f"http://127.0.0.1:47821/#local_secret={SECRET}&local_token={TOKEN}&local_device=owner"
EXTENSION_ID = "jneddlbpgmlheplolfonafkbkbkjhcfj"
PANEL = {"linux:Ctrl+E": {"extension": EXTENSION_ID, "command_name": "_execute_action", "global": False}}
PALETTE = {"linux:Ctrl+Shift+A": {"extension": EXTENSION_ID, "command_name": "open-palette", "global": False}}
SHORTCUT = {**PANEL, **PALETTE}
# Every external program the launcher runs; each is wrapped to record its argv.
WRAPPED = ["awk", "base64", "cat", "chmod", "dirname", "find", "gsettings", "head", "jq", "mkdir", "mktemp", "mv", "readlink", "rm", "rmdir", "sed", "sleep", "stat", "systemctl", "uname"]


def extension_code(n):
    """The fake daemon's one-time code for its n-th local-access call."""
    return f"c{n:063x}"


class Refresh(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.url = None

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if tag == "meta" and values.get("http-equiv") == "refresh":
            self.url = values["content"].removeprefix("0;url=")


class Fixture:
    """A private HOME-like tree with a fake daemon CLI, fake browser and argv-recording wrappers."""

    def __init__(self, root):
        self.root = root
        self.bin = root / "bin"
        self.bin.mkdir(mode=0o700)
        self.launcher = self.bin / "openside-browser"
        shutil.copyfile(ROOT / "tools/openside-browser", self.launcher)
        self.launcher.chmod(0o700)
        self.argv_log = root / "argv.log"
        self.capture = root / "capture.json"
        self.launch_capture = root / "launch.json"
        self.count = root / "local-access-count"
        browser = self.bin / "browser"
        browser.write_text(
            "#!/usr/bin/env python3\n"
            "import json, os, pathlib, stat, sys\n"
            "from urllib.parse import unquote, urlparse\n"
            f"pathlib.Path({str(self.argv_log)!r}).open('a').write('browser ' + ' '.join(sys.argv[1:]) + '\\n')\n"
            "url = next(arg for arg in sys.argv[1:] if arg.startswith('file://'))\n"
            "path = pathlib.Path(unquote(urlparse(url).path))\n"
            f"pathlib.Path({str(self.capture)!r}).write_text(json.dumps({{'argv':sys.argv[1:], 'pid':os.getpid(), 'language':os.environ.get('LANGUAGE'), 'path':str(path), 'html':path.read_text(), 'mode':stat.S_IMODE(path.stat().st_mode), 'parent_mode':stat.S_IMODE(path.parent.stat().st_mode)}}))\n"
            "launch = pathlib.Path(os.environ['XDG_DATA_HOME'], 'openside/extension/launch-pairing.json')\n"
            f"pathlib.Path({str(self.launch_capture)!r}).write_text(json.dumps({{'content':launch.read_text(), 'mode':stat.S_IMODE(launch.stat().st_mode)}} if launch.is_file() and not launch.is_symlink() else None))\n"
        )
        browser.chmod(0o700)
        self.browser = browser
        old_device = {"devices": [{"name": "openside-browser", "device_id": "legacy", "scopes": ["read", "steer"], "kind": "person_device"}]}
        self.person_runtime = root / "daemon-runtime/person-browser"
        access = {"url": URL, "secret": SECRET, "token": TOKEN, "device_id": "owner", "scopes": ["read", "steer", "approve", "human_step", "admin"], "kind": "person_device", "extension_code": "@CODE@", "person_browser_runtime_dir": str(self.person_runtime)}
        cli = self.bin / "openside"
        cli.write_text(
            "#!/bin/bash\n"
            f"printf '%s\\n' \"openside $*\" >>'{self.argv_log}'\n"
            "case \"$*\" in\n"
            f"'--json device list') printf '%s\\n' '{json.dumps(old_device)}' ;;\n"
            f"'--json remote local-access') printf requested > '{root / 'bootstrap-request'}'\n"
            f"  n=$(( $(cat '{self.count}' 2>/dev/null || echo 0) + 1 )); printf '%s' \"$n\" >'{self.count}'\n"
            f"  json='{json.dumps(access)}'; printf -v code 'c%063x' \"$n\"; printf '%s\\n' \"${{json/@CODE@/$code}}\" ;;\n"
            f"'device revoke legacy') printf revoked > '{root / 'unexpected-revoke'}'; exit 3 ;;\n"
            "*) exit 3 ;;\nesac\n"
        )
        cli.chmod(0o700)
        curl = self.bin / "curl"
        curl.write_text(f"#!/bin/sh\nprintf '%s\\n' \"curl $*\" >>'{self.argv_log}'\nexit 0\n")
        curl.chmod(0o700)
        # A desktop notification is recorded, never shown.
        self.notifications = root / "notifications.log"
        notify = self.bin / "notify-send"
        notify.write_text(f"#!/bin/sh\nprintf '%s\\n' \"$*\" >>'{self.notifications}'\n")
        notify.chmod(0o700)
        original = os.environ["PATH"]
        for name in WRAPPED:
            real = shutil.which(name, path=original)
            if real is None:
                continue
            wrapper = self.bin / name
            hooks = {
                # jq can simulate a browser taking the profile right after Preferences were seeded.
                "jq": (
                    "if [[ -n ${OPENSIDE_TEST_LOCK_AFTER_SEED:-} && \"$*\" == *vertical_tabs* ]]; then\n"
                    f"  '{real}' \"$@\"; status=$?; ln -s otherhost-1 \"$OPENSIDE_TEST_LOCK_AFTER_SEED\"; exit $status\nfi\n"
                ),
                # The launch-file cleanup can run soon after the launch instead of 30 s later.
                "sleep": "[[ -n ${OPENSIDE_TEST_FAST_CLEANUP:-} && $1 == 30 ]] && set -- 0.2\n",
                # Once armed, the cleanup's read of its own code starts a newer launch at that exact
                # point, then gives it two seconds to replace the file before the cleanup goes on.
                "cat": (
                    f"if [[ -n ${{OPENSIDE_TEST_RACE_CLEANUP:-}} && $1 == */launch-pairing.json ]] && rm '{self.root / 'race-armed'}' 2>/dev/null; then\n"
                    f"  read_code=$('{real}' \"$@\"); printf '%s\\n' \"$read_code\"\n"
                    f"  env -u OPENSIDE_TEST_RACE_CLEANUP -u OPENSIDE_TEST_FAST_CLEANUP bash '{self.launcher}' </dev/null >/dev/null 2>&1 &\n"
                    "  for _ in $(seq 20); do\n"
                    f"    [[ $('{real}' \"$1\" 2>/dev/null) != \"$read_code\" ]] && : >'{self.root / 'raced'}' && break\n"
                    "    sleep 0.1\n"
                    "  done\n"
                    "  exit 0\n"
                    "fi\n"
                ),
            }
            after = hooks.get(name, "")
            wrapper.write_text(f"#!/bin/bash\nprintf '%s\\n' \"{name} $*\" >>'{self.argv_log}'\n{after}exec '{real}' \"$@\"\n")
            wrapper.chmod(0o700)
        self.config_dir = root / "config" / "openside"
        self.config_dir.mkdir(parents=True)
        (self.config_dir / "config.toml").write_text(f'listen = "127.0.0.1:47821"\nexecutable = "{browser}"\n')
        self.runtime = root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.environment = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"), TMPDIR=str(root), PATH=f"{self.bin}:{original}", OPENSIDE_BROWSER_THEME="dark", XDG_RUNTIME_DIR=str(self.runtime))
        self.environment.pop("LANGUAGE", None)
        self.environment.pop("OPENSIDE_BROWSER_LANG", None)
        self.profile = root / "data/openside/user-browser-profile"
        self.prefs = self.profile / "Default/Preferences"
        self.extension = root / "data/openside/extension"
        self.extension.mkdir(parents=True, mode=0o755)
        (self.extension / "manifest.json").write_text(json.dumps({"manifest_version": 3, "name": "openside", "host_permissions": ["http://127.0.0.1:47820/*"], "externally_connectable": {"matches": ["http://127.0.0.1:47820/*"]}}))
        self.launch_file = self.extension / "launch-pairing.json"

    def run(self):
        return subprocess.run(["bash", str(self.launcher)], env=self.environment, check=True, capture_output=True, text=True, timeout=10)

    def issued(self):
        return int(self.count.read_text()) if self.count.exists() else 0

    def assert_no_secret_in_argv(self, test):
        """No process the launcher started (or the browser) ever saw a credential in its argv."""
        log = self.argv_log.read_text()
        test.assertIn("\njq ", "\n" + log, "the argv wrappers were not used")
        for secret in [SECRET, TOKEN, *[extension_code(n) for n in range(1, self.issued() + 1)]]:
            test.assertNotIn(secret, log)
        test.assertIsNone(re.search(r"c0{40}", log))


class BrowserLauncher(unittest.TestCase):
    def test_handoff_is_private_argv_has_no_credentials_and_legacy_device_is_upgraded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = Fixture(root)
            theme_dir = root / "data/openside/browser-theme-dark"
            handoffs = []
            for runtime_dir in [str(fixture.runtime), None]:
                if runtime_dir:
                    fixture.environment["XDG_RUNTIME_DIR"] = runtime_dir
                else:
                    fixture.environment.pop("XDG_RUNTIME_DIR", None)
                fixture.run()
                result = json.loads(fixture.capture.read_text())
                # The extension's one-time code sits beside its files, private to this user, and names the loopback daemon.
                launch = json.loads(fixture.launch_capture.read_text())
                self.assertEqual(json.loads(launch["content"]), {"url": "http://127.0.0.1:47821", "code": extension_code(fixture.issued())})
                self.assertEqual(launch["mode"], 0o600)
                # The manifest now trusts exactly that daemon origin; no temporary files are left behind.
                manifest = json.loads((fixture.extension / "manifest.json").read_text())
                self.assertEqual(manifest["host_permissions"], ["http://127.0.0.1:47821/*"])
                self.assertEqual(manifest["externally_connectable"]["matches"], ["http://127.0.0.1:47821/*"])
                self.assertEqual(manifest["name"], "openside")
                self.assertEqual(sorted(path.name for path in fixture.extension.iterdir()), ["launch-pairing.json", "manifest.json"])
                # Without an override the browser keeps the OS locale.
                self.assertIsNone(result["language"])
                self.assertEqual(result["mode"], 0o600)
                self.assertEqual(result["parent_mode"], 0o700)
                handoff = Path(result["path"])
                self.assertEqual(handoff.parent.parent, fixture.runtime if runtime_dir else root)
                refresh = Refresh()
                refresh.feed(result["html"])
                self.assertEqual(refresh.url, URL)
                self.assertEqual((root / "bootstrap-request").read_text(), "requested")
                self.assertFalse((root / "unexpected-revoke").exists())
                preferences = json.loads(fixture.prefs.read_text())
                self.assertFalse(preferences["vertical_tabs"]["enabled"])
                self.assertFalse(preferences["auto_pin_new_tab_groups"])
                self.assertFalse(preferences["bookmark_bar"]["show_tab_groups"])
                # Ctrl+E toggles the side panel and Ctrl+Shift+A opens the search palette; Chromium keeps both keys
                # for itself unless the profile binds them.
                self.assertEqual(preferences["extensions"]["commands"], SHORTCUT)
                loaded = next(arg for arg in result["argv"] if arg.startswith("--load-extension="))
                self.assertEqual(loaded, f"--load-extension={fixture.extension},{theme_dir}")
                # The person's browser publishes a person-role gateway (settings and native tools only) for its Default
                # profile in the private directory the daemon names, whatever this process's XDG_RUNTIME_DIR says.
                person_runtime = fixture.person_runtime
                self.assertIn(f"--openside-gateway-runtime-dir={person_runtime}", result["argv"])
                self.assertEqual(stat.S_IMODE(person_runtime.stat().st_mode), 0o700)
                self.assertIn("--openside-gateway-role=person", result["argv"])
                self.assertIn("--profile-directory=Default", result["argv"])
                instance = next(arg for arg in result["argv"] if arg.startswith("--openside-gateway-instance-id="))
                self.assertRegex(instance, r"^--openside-gateway-instance-id=person-[0-9a-f]{16}$")
                self.assertFalse(any(arg.startswith("--openside-agent-session-id") for arg in result["argv"]))
                # The browser's pid (exec keeps the launcher's) and whether it is the patched build, for the
                # daemon to tell a stock browser from a closed one. This fake browser is not patched.
                record = person_runtime / "launch.json"
                self.assertEqual(json.loads(record.read_text()), {"pid": result["pid"], "patched": False})
                self.assertEqual(stat.S_IMODE(record.stat().st_mode), 0o600)
                manifest = json.loads((theme_dir / "manifest.json").read_text())
                self.assertEqual(manifest["name"], "openside evening")
                self.assertEqual(manifest["theme"]["images"]["theme_frame"], "frame.png")
                self.assertEqual((theme_dir / "frame.png").read_bytes()[:8], b"\x89PNG\r\n\x1a\n")
                handoffs.append(handoff)
            # A build that knows the person-role gateway switch is recorded as patched.
            fixture.browser.write_text(fixture.browser.read_text() + "# openside-gateway-role\n")
            fixture.run()
            result = json.loads(fixture.capture.read_text())
            self.assertEqual(json.loads((fixture.person_runtime / "launch.json").read_text()), {"pid": result["pid"], "patched": True})
            handoffs.append(Path(result["path"]))
            # A choice the person made in the browser survives (a removed shortcut included), and the theme can be switched off.
            preferences["auto_pin_new_tab_groups"] = True
            preferences["extensions"]["commands"] = {}
            fixture.prefs.write_text(json.dumps(preferences))
            fixture.environment["OPENSIDE_BROWSER_THEME"] = "off"
            # Chromium on Linux reads its UI language from LANGUAGE, not --lang; browser.language sets
            # it, only from the [browser] table, and OPENSIDE_BROWSER_LANG overrides it. Either is mapped
            # onto a UI locale the browser ships, because Chromium ignores any other LANGUAGE value.
            locales = fixture.bin / "locales"
            locales.mkdir()
            for name in ["en-GB", "en-US", "ko", "pt-BR", "pt-PT", "zh-CN", "zh-TW"]:
                (locales / f"{name}.pak").write_bytes(b"")
            (fixture.config_dir / "config.toml").write_text(
                f'[daemon]\nlisten = "127.0.0.1:47821"\n[other]\nlanguage = "fr"\n[browser] # patched\nexecutable = "{fixture.browser}"\nlanguage = \'ko_KR\'\n'
            )
            fixture.run()
            self.assertEqual(json.loads(fixture.capture.read_text())["language"], "ko")
            handoffs.append(Path(json.loads(fixture.capture.read_text())["path"]))
            for configured, expected in [("pt_BR", "pt-BR"), ("pt-br", "pt-BR"), ("pt_PT", "pt-PT"), ("pt", "pt-BR"), ("zh_Hant", "zh-TW"), ("zh-Hans", "zh-CN"), ("ko-KR", "ko"), ("fr", "fr"), ("en", "en-US")]:
                fixture.environment["OPENSIDE_BROWSER_LANG"] = configured
                fixture.run()
                result = json.loads(fixture.capture.read_text())
                self.assertEqual(result["language"], expected, configured)
                handoffs.append(Path(result["path"]))
            preferences = json.loads(fixture.prefs.read_text())
            self.assertTrue(preferences["auto_pin_new_tab_groups"])
            self.assertEqual(preferences["extensions"]["commands"], {})
            self.assertIn(f"--load-extension={fixture.extension}", result["argv"])
            last = extension_code(fixture.issued())
            self.assertEqual(json.loads(fixture.launch_file.read_text())["code"], last)
            # An earlier launch's cleanup leaves the newer launch's code alone; the newest one removes it.
            checked_newer = False
            deadline = time.monotonic() + 35
            while any(path.exists() for path in [*handoffs, fixture.launch_file]) and time.monotonic() < deadline:
                if not handoffs[0].exists() and not handoffs[1].exists() and handoffs[-1].exists():
                    self.assertEqual(json.loads(fixture.launch_file.read_text())["code"], last)
                    checked_newer = True
                time.sleep(0.1)
            self.assertTrue(checked_newer, "never observed an older cleanup while the newest code was live")
            for path in handoffs:
                self.assertFalse(path.exists(), "one-shot credential handoff was not deleted")
                self.assertFalse(path.parent.exists())
            self.assertFalse(fixture.launch_file.exists(), "the extension's one-time code was not deleted")
            fixture.assert_no_secret_in_argv(self)

    def test_extension_files_never_follow_a_symlink_or_land_in_a_shared_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = Fixture(root)
            victim = root / "victim"
            victim.write_text("keep")
            fixture.launch_file.symlink_to(victim)
            fixture.run()
            self.assertEqual(victim.read_text(), "keep")
            self.assertTrue(fixture.launch_file.is_symlink())
            self.assertIsNone(json.loads(fixture.launch_capture.read_text()))
            fixture.launch_file.unlink()
            # A directory others can write to gets neither the code nor a rewritten manifest.
            before = (fixture.extension / "manifest.json").read_text()
            fixture.extension.chmod(0o775)
            completed = fixture.run()
            self.assertIn("not a private directory", completed.stderr)
            self.assertFalse(fixture.launch_file.exists())
            self.assertEqual((fixture.extension / "manifest.json").read_text(), before)
            fixture.extension.chmod(0o755)
            # A symlinked install (data/extension -> a release) gets its files in the real directory.
            release = root / "release-extension"
            fixture.extension.rename(release)
            fixture.extension.symlink_to(release)
            fixture.run()
            self.assertEqual(json.loads((release / "launch-pairing.json").read_text())["code"], extension_code(fixture.issued()))
            self.assertEqual(json.loads((release / "manifest.json").read_text())["host_permissions"], ["http://127.0.0.1:47821/*"])
            fixture.assert_no_secret_in_argv(self)

    def test_a_running_browser_that_loaded_another_daemon_origin_must_be_restarted(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = Fixture(root)
            fixture.profile.mkdir(parents=True)
            lock = fixture.profile / "SingletonLock"
            loaded = fixture.profile / ".openside-extension-origin"
            live = f"{socket.gethostname()}-{os.getpid()}"
            message = "quit it and open it again"

            def refused():
                completed = subprocess.run(["bash", str(fixture.launcher)], env=fixture.environment, capture_output=True, text=True, timeout=10)
                self.assertEqual(completed.returncode, 1)
                self.assertIn(message, completed.stderr)
                self.assertFalse(fixture.capture.exists(), "the stale browser was handed the launch")
                # No code from this launch is left for the extension (an earlier launch's may still be waiting out its 30 s).
                if fixture.launch_file.exists():
                    self.assertNotEqual(json.loads(fixture.launch_file.read_text())["code"], extension_code(fixture.issued()))

            # A browser that started before any origin was recorded, while the manifest named another port.
            lock.symlink_to(live)
            refused()
            self.assertIn(message, fixture.notifications.read_text())
            self.assertEqual(list(fixture.runtime.iterdir()), [], "the credential handoff outlived the refusal")
            # The rewritten manifest is what the next cold start loads.
            self.assertEqual(json.loads((fixture.extension / "manifest.json").read_text())["host_permissions"], ["http://127.0.0.1:47821/*"])
            self.assertFalse(loaded.exists())
            # A cold start records the origin it loads, and a later launch reuses that browser.
            lock.unlink()
            fixture.run()
            self.assertEqual(loaded.read_text(), "http://127.0.0.1:47821\n")
            lock.symlink_to(live)
            fixture.capture.unlink()
            fixture.run()
            self.assertTrue(fixture.capture.exists())
            # The running browser loaded another daemon origin: refused until it is restarted.
            loaded.write_text("http://127.0.0.1:47999\n")
            fixture.capture.unlink()
            refused()
            # A lock left by a browser that has exited is no running browser.
            gone = subprocess.Popen(["true"])
            gone.wait()
            lock.unlink()
            lock.symlink_to(f"{socket.gethostname()}-{gone.pid}")
            fixture.run()
            self.assertEqual(loaded.read_text(), "http://127.0.0.1:47821\n")
            fixture.assert_no_secret_in_argv(self)

    def test_an_older_launch_cleanup_never_removes_a_newer_launch_code(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = Fixture(root)
            # The first launch's cleanup reads its own code; right then a newer launch starts and tries
            # to install its code before the cleanup removes the file.
            (root / "race-armed").write_text("")
            fixture.environment.update(OPENSIDE_TEST_RACE_CLEANUP="1", OPENSIDE_TEST_FAST_CLEANUP="1")
            fixture.run()
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                if fixture.issued() == 2 and fixture.launch_file.exists() and json.loads(fixture.launch_file.read_text())["code"] == extension_code(2):
                    break
                time.sleep(0.1)
            self.assertFalse((root / "race-armed").exists(), "the cleanup never read its code")
            # The newer launch waited for the cleanup instead of replacing the file under it ...
            self.assertFalse((root / "raced").exists(), "a newer launch replaced the file between the cleanup's check and removal")
            # ... and its code survives the older launch's cleanup.
            self.assertEqual(json.loads(fixture.launch_file.read_text())["code"], extension_code(2))
            self.assertFalse((fixture.extension / ".launch-pairing.lock").exists())
            fixture.assert_no_secret_in_argv(self)

    def test_profile_defaults_are_never_written_while_a_browser_may_own_the_profile(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = Fixture(root)
            marker = fixture.profile / ".openside-panel-shortcut"
            palette_marker = fixture.profile / ".openside-palette-shortcut"
            fixture.profile.mkdir(parents=True)
            # Any lock counts: a stale one, or one from another host, as much as a live one.
            for lock in [lambda: (fixture.profile / "SingletonLock").symlink_to("otherhost-999999"), lambda: (fixture.profile / "SingletonLock").write_text("")]:
                lock()
                fixture.run()
                self.assertFalse(fixture.prefs.exists())
                self.assertFalse(marker.exists())
                self.assertFalse(palette_marker.exists())
                (fixture.profile / "SingletonLock").unlink()
            # Another seeder holding the lock: this launch leaves Preferences alone.
            (fixture.profile / ".openside-seed.lock").mkdir()
            fixture.run()
            self.assertFalse(fixture.prefs.exists())
            (fixture.profile / ".openside-seed.lock").rmdir()
            # A browser that takes the profile while the new file is being prepared still wins.
            fixture.environment["OPENSIDE_TEST_LOCK_AFTER_SEED"] = str(fixture.profile / "SingletonLock")
            fixture.run()
            self.assertFalse(fixture.prefs.exists())
            self.assertFalse(marker.exists())
            self.assertFalse(palette_marker.exists())
            self.assertEqual([path.name for path in (fixture.profile / "Default").iterdir()], [])
            del fixture.environment["OPENSIDE_TEST_LOCK_AFTER_SEED"]
            (fixture.profile / "SingletonLock").unlink()
            # Unreadable Preferences are left as they are, and the shortcut is not marked as seeded.
            fixture.prefs.write_text("{not json")
            fixture.run()
            self.assertEqual(fixture.prefs.read_text(), "{not json")
            self.assertFalse(marker.exists())
            self.assertFalse(palette_marker.exists())
            fixture.prefs.unlink()
            fixture.run()
            self.assertEqual(json.loads(fixture.prefs.read_text())["extensions"]["commands"], SHORTCUT)
            self.assertTrue(marker.exists())
            self.assertTrue(palette_marker.exists())
            self.assertFalse((fixture.profile / ".openside-seed.lock").exists())
            fixture.assert_no_secret_in_argv(self)

    def test_the_palette_shortcut_reaches_profiles_seeded_before_it_and_never_takes_a_held_key(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = Fixture(root)
            (fixture.profile / "Default").mkdir(parents=True)
            # A profile seeded before the palette existed, whose person removed the panel shortcut: only the
            # palette is added, and everything already there stays.
            (fixture.profile / ".openside-panel-shortcut").write_text("")
            fixture.prefs.write_text(json.dumps({"vertical_tabs": {"enabled": True}, "extensions": {"commands": {}}}))
            fixture.run()
            preferences = json.loads(fixture.prefs.read_text())
            self.assertEqual(preferences["extensions"]["commands"], PALETTE)
            self.assertTrue(preferences["vertical_tabs"]["enabled"])
            self.assertTrue((fixture.profile / ".openside-palette-shortcut").exists())
            # Once seeded, a palette shortcut the person removed is not brought back.
            preferences["extensions"]["commands"] = {}
            fixture.prefs.write_text(json.dumps(preferences))
            fixture.run()
            self.assertEqual(json.loads(fixture.prefs.read_text())["extensions"]["commands"], {})
            # A key the person gave to another extension keeps that binding; the profile still counts as seeded.
            (fixture.profile / ".openside-panel-shortcut").unlink()
            (fixture.profile / ".openside-palette-shortcut").unlink()
            other = {"linux:Ctrl+Shift+A": {"extension": "a" * 32, "command_name": "search", "global": False}}
            preferences["extensions"]["commands"] = other
            fixture.prefs.write_text(json.dumps(preferences))
            fixture.run()
            self.assertEqual(json.loads(fixture.prefs.read_text())["extensions"]["commands"], {**PANEL, **other})
            self.assertTrue((fixture.profile / ".openside-palette-shortcut").exists())
            fixture.assert_no_secret_in_argv(self)

    def test_the_person_gateway_goes_only_where_the_daemon_says(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fixture = Fixture(root)
            cli = fixture.bin / "openside"
            named = f', "person_browser_runtime_dir": "{fixture.person_runtime}"'
            original = cli.read_text()
            self.assertIn(named, original)
            # A daemon that names no directory (or a relative one) gets a browser without a gateway, never a guessed place.
            for replacement in ["", ', "person_browser_runtime_dir": "relative/person-browser"']:
                cli.write_text(original.replace(named, replacement))
                fixture.run()
                argv = json.loads(fixture.capture.read_text())["argv"]
                self.assertFalse(any(arg.startswith("--openside-gateway-") for arg in argv), argv)
                self.assertNotIn("--profile-directory=Default", argv)
                self.assertFalse(fixture.person_runtime.exists())
                self.assertFalse((fixture.runtime / "openside").exists())
            fixture.assert_no_secret_in_argv(self)

    def test_a_person_cannot_pass_a_test_only_switch(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Fixture(Path(directory))
            switches = ["--openside-test-tab-rename", "--openside-test-anything=1"]
            completed = subprocess.run(
                ["bash", str(fixture.launcher), switches[0], "https://example.com/", switches[1]],
                env=fixture.environment, check=True, capture_output=True, text=True, timeout=10,
            )
            for switch in switches:
                self.assertIn(f"ignoring the test-only switch {switch}", completed.stderr)
            argv = json.loads(fixture.capture.read_text())["argv"]
            self.assertFalse(any(arg.startswith("--openside-test-") for arg in argv), argv)
            self.assertIn("https://example.com/", argv)


if __name__ == "__main__":
    unittest.main()

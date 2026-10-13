"""Hermetic desktop launcher checks; no user daemon or browser is started."""
import html.parser
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
SECRET = "a" * 64
TOKEN = "b" * 64
URL = f"http://127.0.0.1:47821/#local_secret={SECRET}&local_token={TOKEN}&local_device=owner"


class Refresh(html.parser.HTMLParser):
    def __init__(self):
        super().__init__()
        self.url = None

    def handle_starttag(self, tag, attrs):
        values = dict(attrs)
        if tag == "meta" and values.get("http-equiv") == "refresh":
            self.url = values["content"].removeprefix("0;url=")


class BrowserLauncher(unittest.TestCase):
    def test_handoff_is_private_argv_has_no_credentials_and_legacy_device_is_upgraded(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            bin_dir = root / "bin"
            bin_dir.mkdir(mode=0o700)
            launcher = bin_dir / "openside-browser"
            shutil.copyfile(ROOT / "tools/openside-browser", launcher)
            launcher.chmod(0o700)
            fake_browser = bin_dir / "browser"
            capture = root / "capture.json"
            fake_browser.write_text(
                "#!/usr/bin/env python3\n"
                "import json, pathlib, stat, sys\n"
                "from urllib.parse import unquote, urlparse\n"
                "url = next(arg for arg in sys.argv[1:] if arg.startswith('file://'))\n"
                "path = pathlib.Path(unquote(urlparse(url).path))\n"
                f"pathlib.Path({str(capture)!r}).write_text(json.dumps({{'argv':sys.argv[1:], 'path':str(path), 'html':path.read_text(), 'mode':stat.S_IMODE(path.stat().st_mode), 'parent_mode':stat.S_IMODE(path.parent.stat().st_mode)}}))\n"
            )
            fake_browser.chmod(0o700)
            cli = bin_dir / "openside"
            old_device = {"devices": [{"name": "openside-browser", "device_id": "legacy", "scopes": ["read", "steer"], "kind": "person_device"}]}
            access = {"url": URL, "secret": SECRET, "token": TOKEN, "device_id": "owner", "scopes": ["read", "steer", "approve", "human_step", "admin"], "kind": "person_device"}
            cli.write_text(
                "#!/bin/bash\ncase \"$*\" in\n"
                f"'--json device list') printf '%s\\n' '{json.dumps(old_device)}' ;;\n"
                f"'--json remote local-access') printf requested > '{root / 'bootstrap-request'}'; printf '%s\\n' '{json.dumps(access)}' ;;\n"
                f"'device revoke legacy') printf revoked > '{root / 'unexpected-revoke'}'; exit 3 ;;\n"
                "*) exit 3 ;;\nesac\n"
            )
            cli.chmod(0o700)
            curl = bin_dir / "curl"
            curl.write_text("#!/bin/sh\nexit 0\n")
            curl.chmod(0o700)
            config_dir = root / "config" / "openside"
            config_dir.mkdir(parents=True)
            (config_dir / "config.toml").write_text(f'listen = "127.0.0.1:47821"\nexecutable = "{fake_browser}"\n')
            runtime = root / "runtime"
            runtime.mkdir(mode=0o700)
            environment = dict(os.environ, XDG_CONFIG_HOME=str(root / "config"), XDG_DATA_HOME=str(root / "data"), TMPDIR=str(root), PATH=f"{bin_dir}:{os.environ['PATH']}", OPENSIDE_BROWSER_THEME="dark")
            prefs_path = root / "data/openside/user-browser-profile/Default/Preferences"
            theme_dir = root / "data/openside/browser-theme-dark"
            handoffs = []
            for runtime_dir in [str(runtime), None]:
                if runtime_dir:
                    environment["XDG_RUNTIME_DIR"] = runtime_dir
                else:
                    environment.pop("XDG_RUNTIME_DIR", None)
                subprocess.run(["bash", str(launcher)], env=environment, check=True, capture_output=True, timeout=10)
                result = json.loads(capture.read_text())
                self.assertFalse(any(SECRET in arg or TOKEN in arg for arg in result["argv"]))
                self.assertEqual(result["mode"], 0o600)
                self.assertEqual(result["parent_mode"], 0o700)
                handoff = Path(result["path"])
                self.assertEqual(handoff.parent.parent, runtime if runtime_dir else root)
                refresh = Refresh()
                refresh.feed(result["html"])
                self.assertEqual(refresh.url, URL)
                self.assertEqual((root / "bootstrap-request").read_text(), "requested")
                self.assertFalse((root / "unexpected-revoke").exists())
                preferences = json.loads(prefs_path.read_text())
                self.assertFalse(preferences["vertical_tabs"]["enabled"])
                self.assertFalse(preferences["auto_pin_new_tab_groups"])
                self.assertFalse(preferences["bookmark_bar"]["show_tab_groups"])
                loaded = next(arg for arg in result["argv"] if arg.startswith("--load-extension="))
                self.assertEqual(loaded, f"--load-extension={root / 'data/openside/extension'},{theme_dir}")
                manifest = json.loads((theme_dir / "manifest.json").read_text())
                self.assertEqual(manifest["name"], "openside evening")
                self.assertEqual(manifest["theme"]["images"]["theme_frame"], "frame.png")
                self.assertEqual((theme_dir / "frame.png").read_bytes()[:8], b"\x89PNG\r\n\x1a\n")
                handoffs.append(handoff)
            # A choice the person made in the browser survives, and the theme can be switched off.
            preferences["auto_pin_new_tab_groups"] = True
            prefs_path.write_text(json.dumps(preferences))
            environment["OPENSIDE_BROWSER_THEME"] = "off"
            subprocess.run(["bash", str(launcher)], env=environment, check=True, capture_output=True, timeout=10)
            result = json.loads(capture.read_text())
            self.assertTrue(json.loads(prefs_path.read_text())["auto_pin_new_tab_groups"])
            self.assertIn(f"--load-extension={root / 'data/openside/extension'}", result["argv"])
            handoffs.append(Path(result["path"]))
            deadline = time.monotonic() + 35
            while any(path.exists() for path in handoffs) and time.monotonic() < deadline:
                time.sleep(0.1)
            for path in handoffs:
                self.assertFalse(path.exists(), "one-shot credential handoff was not deleted")
                self.assertFalse(path.parent.exists())


if __name__ == "__main__":
    unittest.main()

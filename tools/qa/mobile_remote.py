#!/usr/bin/env python3
"""Run the ignored QR-to-phone journey twice and retain strict gate evidence."""

from __future__ import annotations

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
TESTS = {
    "relay": "relay_phone_qr_chat_approval_preview_attachment_and_notification",
    "cloudflare": "cloudflare_quick_phone_qr_chat_approval_preview_attachment_and_notification",
}


def passed(output: str, returncode: int, transport: str) -> bool:
    test = TESTS[transport]
    return (
        returncode == 0
        and re.search(rf"(?m)^test {test} \.\.\.", output) is not None
        and "test result: ok. 1 passed; 0 failed; 0 ignored;" in output
        and all(f"MOBILE_REMOTE {transport} {profile} PASS " in output
                for profile in ("iphone", "android"))
        and re.search(r"(?im)\bskip:", output) is None
    )


def run(directory: Path, jobs: int, transport: str) -> int:
    directory.mkdir(parents=True, exist_ok=True)
    test = TESTS[transport]
    suite = ET.Element("testsuite", name=f"mobile-remote-{transport}", tests="2")
    failures = 0
    for repetition in (1, 2):
        environment = os.environ.copy()
        environment["OPENSIDE_MOBILE_CAPTURES"] = str(directory / "mobile-remote-captures" / f"run{repetition}")
        command = [
            "cargo", "test", "--locked", "-j", str(jobs), "-p", "openside-e2e",
            "--test", "mobile_remote", test, "--", "--ignored", "--exact",
            "--nocapture", "--test-threads=1", "--color=never",
        ]
        started = time.monotonic()
        try:
            result = subprocess.run(command, cwd=ROOT, env=environment, stdout=subprocess.PIPE,
                                    stderr=subprocess.STDOUT, text=True)
            output, returncode = result.stdout, result.returncode
        except OSError as error:
            output, returncode = f"Could not execute Cargo: {error}\n", 1
        duration = time.monotonic() - started
        sys.stdout.write(output)
        (directory / f"mobile-remote-{transport}-run{repetition}.log").write_text(output)
        case = ET.SubElement(suite, "testcase", classname="crates.openside-e2e.tests.mobile_remote",
                             name=test, time=f"{duration:.6f}")
        if not passed(output, returncode, transport):
            failures += 1
            ET.SubElement(case, "failure", message=f"Mobile journey failed or did not run (Cargo rc={returncode}, repetition={repetition})")
        ET.SubElement(case, "system-out").text = output
    suite.set("failures", str(failures))
    ET.ElementTree(suite).write(directory / f"mobile-remote-{transport}.xml", encoding="utf-8", xml_declaration=True)
    return int(failures != 0)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=2)
    parser.add_argument("--transport", choices=tuple(TESTS), default="relay")
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    if args.transport == "cloudflare" and os.environ.get("OPENSIDE_MOBILE_CLOUDFLARE") != "1":
        parser.error("Cloudflare requires OPENSIDE_MOBILE_CLOUDFLARE=1 public-network opt-in")
    return run(args.results, args.jobs, args.transport)


if __name__ == "__main__":
    raise SystemExit(main())

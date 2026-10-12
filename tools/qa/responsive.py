#!/usr/bin/env python3
"""Run the ignored first-browser-action measurement and retain gate evidence."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
TEST = "cold_and_warm_first_browser_action"


def run(directory: Path, jobs: int) -> int:
    directory.mkdir(parents=True, exist_ok=True)
    command = [
        "cargo", "test", "--locked", "-j", str(jobs), "-p", "openside-e2e",
        "--test", "responsive", TEST, "--", "--ignored", "--exact",
        "--nocapture", "--test-threads=2", "--color=never",
    ]
    started = time.monotonic()
    try:
        completed = subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE,
                                   stderr=subprocess.STDOUT, text=True)
        output, returncode = completed.stdout, completed.returncode
    except OSError as error:
        output, returncode = f"Could not execute Cargo: {error}\n", 1
    duration = time.monotonic() - started
    sys.stdout.write(output)
    (directory / "responsive.log").write_text(output)
    # A missing browser can return from the Rust test with an `ok` result; that is not evidence.
    passed = (
        returncode == 0
        and re.search(rf"(?m)^test {TEST} \.\.\.", output) is not None
        and re.search(r"(?m)^test result: ok\. 1 passed; 0 failed; 0 ignored;", output) is not None
        and "RESPONSIVE " in output
        and re.search(r"(?im)\bskip:", output) is None
    )
    suite = ET.Element("testsuite", name="responsive", tests="1",
                       failures="0" if passed else "1", time=f"{duration:.6f}")
    case = ET.SubElement(suite, "testcase", classname="crates.openside-e2e.tests.responsive",
                         name=TEST, time=f"{duration:.6f}")
    if not passed:
        ET.SubElement(case, "failure", message=f"Responsive measurement failed or did not run (Cargo rc={returncode})")
    ET.SubElement(case, "system-out").text = output
    ET.ElementTree(suite).write(directory / "responsive.xml", encoding="utf-8", xml_declaration=True)
    return 0 if passed else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--results", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=4)
    args = parser.parse_args()
    if args.jobs < 1:
        parser.error("--jobs must be positive")
    return run(args.results, args.jobs)


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Run only the real-process tests declared in the docs/specs/e2e-scenarios.md scenario table."""

from __future__ import annotations

import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "docs/specs/e2e-scenarios.md"
LOG = ROOT / "target/e2e-scenarios.log"
TEST_FN = re.compile(
    r"#\[(?:tokio::)?test(?:\([^\]]*\))?\]\s*"
    r"(?:#\[[^\]]*\]\s*)*(?:async\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\("
)
IDENTIFIER = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
CRATE = re.compile(r"[a-z][a-z0-9-]*")


@dataclass(frozen=True)
class Scenario:
    label: str
    crate: str
    target: str
    test: str


def scenarios(manifest: Path = MANIFEST) -> list[Scenario]:
    rows = []
    in_table = False
    for line in manifest.read_text().splitlines():
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if cells == ["Scenario", "Crate", "Target", "Test"]:
            in_table = True
            continue
        if not in_table:
            continue
        if not line.startswith("|"):
            break
        if all(re.fullmatch(r":?-+:?", cell) for cell in cells):
            continue
        if len(cells) != 4:
            raise ValueError(f"invalid scenario table row: {line}")
        row = Scenario(*cells)
        if not row.label or not CRATE.fullmatch(row.crate):
            raise ValueError(f"invalid scenario/crate: {line}")
        if not IDENTIFIER.fullmatch(row.target) or not IDENTIFIER.fullmatch(row.test):
            raise ValueError(f"invalid test target/name: {line}")
        rows.append(row)
    if not rows:
        raise ValueError("the canonical scenario table is empty or missing")
    selectors = [(row.crate, row.target, row.test) for row in rows]
    if len(set(selectors)) != len(selectors):
        raise ValueError("a test is listed more than once in the scenario table")
    return rows


def cargo_commands(rows: list[Scenario], root: Path = ROOT) -> dict[str, list[str]]:
    commands = {}
    for crate in dict.fromkeys(row.crate for row in rows):
        selected = [row for row in rows if row.crate == crate]
        targets = list(dict.fromkeys(row.target for row in selected))
        requested = {row.test for row in selected}
        all_names: set[str] = set()
        for target in targets:
            source = root / "crates" / crate / "tests" / f"{target}.rs"
            names = set(TEST_FN.findall(source.read_text()))
            if all_names & names:
                raise ValueError(f"ambiguous test names across {crate} targets")
            all_names.update(names)
            for row in selected:
                if row.target == target and row.test not in names:
                    raise ValueError(f"listed test does not exist: {source.relative_to(root)}::{row.test}")
        command = ["cargo", "test", "-j", "8", "-p", crate]
        for target in targets:
            command.extend(["--test", target])
        command.extend(["--no-fail-fast", "--", "--exact", "--nocapture", "--test-threads=1", "--color=never"])
        # libtest has one positive filter; exact negative filters select a disjoint list
        # across several integration targets in a single Cargo invocation.
        for name in sorted(all_names - requested):
            command.extend(["--skip", name])
        commands[crate] = command
    return commands


def outcomes(text: str) -> dict[str, str]:
    found = {}
    current: str | None = None
    skipped = False
    pending_skip = False
    for line in text.splitlines():
        started = re.match(r"^test ([A-Za-z_][A-Za-z0-9_:]*) \.\.\.\s*(.*)$", line)
        if started:
            current, line = started.groups()
            skipped = pending_skip
            pending_skip = False
        if current is None:
            pending_skip |= "skip:" in line.lower()
            continue
        if "skip:" in line.lower():
            skipped = True
        status = re.search(r"(?:^|\s)(ok|FAILED|ignored)\s*$", line)
        if status:
            found[current] = "PASS" if status[1] == "ok" and not skipped else "FAIL"
            current = None
    return found


def run() -> int:
    LOG.parent.mkdir(parents=True, exist_ok=True)
    with LOG.open("w") as log:
        def emit(text: str) -> None:
            sys.stdout.write(text)
            sys.stdout.flush()
            log.write(text)
            log.flush()

        try:
            rows = scenarios()
            commands = cargo_commands(rows)
        except (OSError, ValueError) as error:
            emit(f"FAIL: {error}\n")
            return 1
        results: dict[tuple[str, str], str] = {}
        failed = False
        for crate, command in commands.items():
            emit(f"\n$ {' '.join(command)}\n")
            output = []
            try:
                with subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True) as process:
                    assert process.stdout is not None
                    for line in process.stdout:
                        emit(line)
                        output.append(line)
                    returncode = process.wait()
            except OSError as error:
                emit(f"FAIL: could not execute Cargo: {error}\n")
                returncode = 1
            crate_results = outcomes("".join(output))
            expected = {row.test for row in rows if row.crate == crate}
            unexpected = set(crate_results) - expected
            if unexpected:
                emit(f"FAIL: unlisted tests executed in {crate}: {sorted(unexpected)}\n")
            failed |= returncode != 0 or bool(unexpected)
            for name in expected:
                results[crate, name] = crate_results.get(name, "FAIL")
        emit("\n| Scenario | Test | Result |\n| --- | --- | --- |\n")
        for row in rows:
            status = results[row.crate, row.test]
            emit(f"| {row.label} | {row.crate}/{row.target}::{row.test} | {status} |\n")
            failed |= status != "PASS"
        emit(f"\nScenario suite: {'FAIL' if failed else 'PASS'}; {len(rows)} listed tests.\n")
        emit("Full output: target/e2e-scenarios.log\n")
        return int(failed)


if __name__ == "__main__":
    sys.exit(run())

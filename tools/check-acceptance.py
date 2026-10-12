#!/usr/bin/env python3
"""Validate docs/specs/acceptance.csv against the test tree and (optionally) test-run evidence.

Static checks (always): header, status vocabulary, and that every `pass` row names tests that are
actually defined. With --results DIR every `pass` reference must additionally be found passed in
the evidence files written by tools/gate.sh. With --final the exception budget and the
user-action notes are enforced too.

Exit status is 0 iff no ERROR line is printed. Python 3 standard library only.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import re
import sys
import xml.etree.ElementTree as ET
from dataclasses import dataclass, field
from pathlib import Path

HEADER = ["id", "source", "milestone", "acceptance_criterion", "test_path", "status", "note"]
STATUSES = ["pass", "unverified", "out-of-scope", "blocked-external"]
EXCEPTION_STATUSES = ("out-of-scope", "blocked-external")
EXCEPTION_RATIO = 0.10
USER_ACTION_RE = re.compile(r"(사용자 할 일:|user action:)", re.IGNORECASE)
TS_SUFFIXES = (".ts", ".tsx", ".js", ".mjs")
ATTR_WINDOW = 6

# --------------------------------------------------------------------------------------------
# Test-definition detection
# --------------------------------------------------------------------------------------------

_CHAR_LITERAL = re.compile(r"'(?:\\(?:x[0-9a-fA-F]{2}|u\{[0-9a-fA-F_]+\}|.)|[^'\\\n])'")
_RAW_STRING_START = re.compile(r"b?r(#*)\"")


def mask_rust(src: str) -> str:
    """Blank comments and string/char literals (newlines kept) so brace and keyword scans are safe."""
    out = list(src)
    n = len(src)

    def blank(a: int, b: int) -> None:
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    i = 0
    while i < n:
        c = src[i]
        if src.startswith("//", i):
            j = src.find("\n", i)
            j = n if j < 0 else j
            blank(i, j)
            i = j
        elif src.startswith("/*", i):
            depth, j = 1, i + 2
            while j < n and depth:
                if src.startswith("/*", j):
                    depth += 1
                    j += 2
                elif src.startswith("*/", j):
                    depth -= 1
                    j += 2
                else:
                    j += 1
            blank(i, j)
            i = j
        elif (c in "rb") and not (i and (src[i - 1].isalnum() or src[i - 1] == "_")) and (
            m := _RAW_STRING_START.match(src, i)
        ):
            closing = '"' + m.group(1)
            j = src.find(closing, m.end())
            j = n if j < 0 else j + len(closing)
            blank(i, j)
            i = j
        elif c == '"':
            j = i + 1
            while j < n and src[j] != '"':
                j += 2 if src[j] == "\\" else 1
            j = min(j + 1, n)
            blank(i, j)
            i = j
        elif c == "'":
            m = _CHAR_LITERAL.match(src, i)
            if m:
                blank(i, m.end())
                i = m.end()
            else:
                i += 1  # lifetime
        else:
            i += 1
    return "".join(out)


def _proptest_spans(masked: str) -> list[tuple[int, int]]:
    spans = []
    for m in re.finditer(r"\bproptest\s*!\s*([{(\[])", masked):
        depth, j = 0, m.start(1)
        while j < len(masked):
            ch = masked[j]
            if ch in "{([":
                depth += 1
            elif ch in "})]":
                depth -= 1
                if depth == 0:
                    break
            j += 1
        spans.append((m.start(1), j))
    return spans


def rust_defines_test(src: str, name: str) -> bool:
    masked = mask_rust(src)
    lines = masked.split("\n")
    line_starts, pos = [], 0
    for ln in lines:
        line_starts.append(pos)
        pos += len(ln) + 1
    spans = _proptest_spans(masked)
    for m in re.finditer(r"\bfn\s+" + re.escape(name) + r"\s*[(<]", masked):
        if any(a < m.start() < b for a, b in spans):
            return True
        lineno = max(k for k, s in enumerate(line_starts) if s <= m.start())
        seen = 0
        for k in range(lineno - 1, -1, -1):
            text = lines[k].strip()
            if not text:
                continue
            seen += 1
            if text.startswith("#[") and "test" in text:
                return True
            if re.search(r"\bfn\b|[;}]$", text):
                break  # the attribute above belongs to an earlier item
            if seen >= ATTR_WINDOW:
                break
    return False


def python_defines_test(src: str, name: str) -> bool:
    base = name.split("[", 1)[0].split("::")[-1]
    return re.search(r"^\s*(?:async\s+)?def\s+" + re.escape(base) + r"\s*\(", src, re.MULTILINE) is not None


_TS_CALL = re.compile(r"""\b(it|test|describe)(?:\.only)?\s*\(\s*(['"`])((?:\\.|(?!\2).)*)\2""", re.DOTALL)


def ts_defines_test(src: str, name: str) -> bool:
    tests, suites = set(), set()
    for m in _TS_CALL.finditer(src):
        (suites if m.group(1) == "describe" else tests).add(m.group(3))
    *parents, leaf = name.split(" > ")
    return leaf in tests and all(p in suites for p in parents)


# --------------------------------------------------------------------------------------------
# Evidence parsing
# --------------------------------------------------------------------------------------------

_RUNNING = re.compile(r"^\s*Running\s+(?:unittests\s+)?(\S+)\s+\((\S+)\)\s*$")
_DOCTESTS = re.compile(r"^\s*Doc-tests\s+\S+")
_RESULT = re.compile(r"^test (.+?)(?: - should panic)? \.\.\. (ok|FAILED|ignored|bench)\b")


@dataclass
class CargoSection:
    src_path: str
    binary: str  # file name of the deps binary without the hash, hyphens normalised to underscores
    results: dict[str, str] = field(default_factory=dict)


def parse_cargo_log(text: str) -> list[CargoSection]:
    sections: list[CargoSection] = []
    current: CargoSection | None = None
    for line in text.splitlines():
        m = _RUNNING.match(line)
        if m:
            deps = m.group(2).rsplit("/", 1)[-1]
            binary = re.sub(r"-[0-9a-f]+$", "", deps).replace("-", "_")
            current = CargoSection(m.group(1), binary)
            sections.append(current)
            continue
        if _DOCTESTS.match(line):
            current = None
            continue
        m = _RESULT.match(line)
        if m and current is not None:
            current.results[m.group(1)] = m.group(2)
    return sections


def _name_matches(result_name: str, wanted: str) -> bool:
    return result_name == wanted or result_name.endswith("::" + wanted)


def parse_junit(text: str, root: Path, prefixes: tuple[str, ...] = ("agent/", "")) -> dict[tuple[str, str], bool]:
    """(repo-relative file, test name) -> passed. A repeated key is passed only if every case passed."""
    out: dict[tuple[str, str], bool] = {}
    tree = ET.fromstring(text)
    for case in tree.iter("testcase"):
        classname, name = case.get("classname", ""), case.get("name", "")
        parts = classname.split(".") if classname else []
        # pytest classnames are relative to the project it ran in: agent/ and bench/ run from
        # their own directory, tools/ from the repo root.
        path, rest = prefixes[0] + "/".join(parts) + ".py", []
        found = False
        for prefix in prefixes:
            for cut in range(len(parts), 0, -1):
                candidate = prefix + "/".join(parts[:cut]) + ".py"
                if (root / candidate).is_file():
                    path, rest, found = candidate, parts[cut:], True
                    break
            if found:
                break
        full = "::".join([*rest, name])
        ok = not any(case.find(t) is not None for t in ("failure", "error", "skipped"))
        out[(path, full)] = out.get((path, full), True) and ok
    return out


def parse_vitest(text: str) -> list[tuple[str, dict[str, bool]]]:
    data = json.loads(text)
    files = []
    for file_result in data.get("testResults", []):
        names: dict[str, bool] = {}
        for a in file_result.get("assertionResults", []):
            ok = a.get("status") == "passed"
            title = a.get("title", "")
            candidates = {title, a.get("fullName", ""), " > ".join([*a.get("ancestorTitles", []), title])}
            for cand in candidates:
                if cand:
                    names[cand] = names.get(cand, True) and ok
        files.append((file_result.get("name", ""), names))
    return files


class Evidence:
    def __init__(self, directory: Path, root: Path):
        self.directory = directory
        self.root = root
        self.cargo: list[CargoSection] = []
        self.junit: dict[tuple[str, str], bool] = {}
        self.vitest: list[tuple[str, dict[str, bool]]] = []
        self.scripts: dict[str, bool] = {}
        self.files_loaded: list[str] = []
        self.problems: list[str] = []
        self._load()

    def _load(self) -> None:
        if not self.directory.is_dir():
            return
        for p in sorted(self.directory.iterdir()):
            if not p.is_file():
                continue
            try:
                if p.name == "scripts.txt":
                    for line in p.read_text(errors="replace").splitlines():
                        verdict, _, path = line.strip().partition(" ")
                        if verdict in ("PASS", "FAIL") and path:
                            self.scripts[path.strip()] = self.scripts.get(path.strip(), True) and verdict == "PASS"
                elif p.suffix in (".log", ".txt"):
                    self.cargo.extend(parse_cargo_log(p.read_text(errors="replace")))
                elif p.suffix == ".xml":
                    # bench/ is a separate uv project whose pytest classnames start at bench/.
                    prefixes = ("bench/",) if p.name.startswith("bench") else ("agent/", "")
                    for key, ok in parse_junit(p.read_text(errors="replace"), self.root, prefixes).items():
                        self.junit[key] = self.junit.get(key, True) and ok
                elif p.name.endswith(".vitest.json"):
                    self.vitest.extend(parse_vitest(p.read_text(errors="replace")))
                else:
                    continue
            except (ET.ParseError, json.JSONDecodeError, OSError) as e:
                self.problems.append(f"cannot parse evidence file {p.name}: {e}")
                continue
            self.files_loaded.append(p.name)

    # Each lookup returns "pass" | "fail" | "missing" plus a short detail for error lines.
    def check(self, path: str, name: str | None) -> tuple[str, str]:
        if name is None:
            if path not in self.scripts:
                return "missing", "no PASS/FAIL line in scripts.txt"
            return ("pass", "") if self.scripts[path] else ("fail", "script FAILED in scripts.txt")
        if path.endswith(".rs"):
            return self._check_rust(path, name)
        if path.endswith(".py"):
            return self._check_python(path, name)
        return self._check_vitest(path, name)

    def _check_rust(self, path: str, name: str) -> tuple[str, str]:
        parts = Path(path).parts
        if len(parts) < 4 or parts[0] != "crates" or parts[2] not in ("src", "tests"):
            return "fail", "unsupported rust test layout (expected crates/<crate>/{src,tests}/...)"
        crate = parts[1].replace("-", "_")
        if parts[2] == "src":
            sections = [s for s in self.cargo if s.binary == crate]
        else:
            stem = Path(path).stem.replace("-", "_")
            rel = "/".join(parts[2:])
            sections = [s for s in self.cargo if s.src_path == rel or s.src_path.endswith("/" + rel)]
            # Same-named integration files in different crates cannot be told apart from the text log;
            # the deps binary name is the file stem, so prefer exact binary matches first.
            sections.sort(key=lambda s: s.binary != stem)
        statuses = [st for s in sections for r, st in s.results.items() if _name_matches(r, name)]
        if "ok" in statuses:
            return "pass", ""
        if statuses:
            return "fail", f"cargo reports {'/'.join(sorted(set(statuses)))}"
        return "missing", "no matching `test ... ok` line in cargo test logs"

    def _check_python(self, path: str, name: str) -> tuple[str, str]:
        if "[" in name:
            hits = [self.junit[(path, name)]] if (path, name) in self.junit else []
        else:
            hits = [ok for (p, n), ok in self.junit.items() if p == path and n.split("[", 1)[0] == name]
        if not hits:
            return "missing", "no matching testcase in JUnit XML"
        return ("pass", "") if all(hits) else ("fail", "JUnit reports failure/error/skip")

    def _check_vitest(self, path: str, name: str) -> tuple[str, str]:
        hits = []
        for abs_path, names in self.vitest:
            norm = abs_path.replace("\\", "/")
            if (norm == path or norm.endswith("/" + path)) and name in names:
                hits.append(names[name])
        if not hits:
            return "missing", "no matching assertion in vitest JSON"
        return ("pass", "") if all(hits) else ("fail", "vitest reports non-passed status")


# --------------------------------------------------------------------------------------------
# Row validation
# --------------------------------------------------------------------------------------------


@dataclass
class Report:
    total: int = 0
    counts: dict[str, int] = field(default_factory=lambda: {s: 0 for s in STATUSES})
    errors: list[tuple[int, str, str]] = field(default_factory=list)
    results_checked: bool = False
    final: bool = False

    def err(self, row: int, rid: str, reason: str) -> None:
        self.errors.append((row, rid, reason))


def split_entries(test_path: str) -> list[str]:
    return [e.strip() for e in test_path.split(";") if e.strip()]


def _check_definition(root: Path, path: str, name: str | None) -> str | None:
    """Return an error reason, or None when the entry is statically sound."""
    rel = Path(path)
    if rel.is_absolute() or ".." in rel.parts:
        return f"{path}: must be a repo-relative path"
    file = root / rel
    if not file.is_file():
        return f"{path}: file does not exist"
    if name is None:
        return None if os.access(file, os.X_OK) else f"{path}: bare path must be an executable script"
    src = file.read_text(errors="replace")
    if path.endswith(".rs"):
        ok = rust_defines_test(src, name.split("::")[-1])
    elif path.endswith(".py"):
        ok = python_defines_test(src, name)
    elif path.endswith(TS_SUFFIXES):
        ok = ts_defines_test(src, name)
    else:
        return f"{path}::{name}: unsupported test file type"
    return None if ok else f"{path}::{name}: no test definition found"


def check_pass_row(root: Path, row: int, rid: str, test_path: str, evidence: Evidence | None, rep: Report) -> None:
    entries = split_entries(test_path)
    if not entries:
        rep.err(row, rid, "pass row has empty test_path")
        return
    for entry in entries:
        path, sep, name = entry.partition("::")
        path, name_or_none = path.strip(), (name.strip() if sep else None)
        if sep and not name_or_none:
            rep.err(row, rid, f"{entry}: empty test name")
            continue
        reason = _check_definition(root, path, name_or_none)
        if reason:
            rep.err(row, rid, reason)
            continue
        if evidence is not None:
            verdict, detail = evidence.check(path, name_or_none)
            label = entry if name_or_none is None else f"{path}::{name_or_none}"
            if verdict == "missing":
                rep.err(row, rid, f"{label}: not found in results ({detail})")
            elif verdict == "fail":
                rep.err(row, rid, f"{label}: not passed ({detail})")


def check_exception_note(row: int, rid: str, status: str, note: str, rep: Report) -> None:
    m = USER_ACTION_RE.search(note)
    if not m:
        rep.err(row, rid, f"{status} note needs a reason followed by '사용자 할 일:' or 'User action:' marker")
        return
    reason, action = note[: m.start()].strip(), note[m.end() :].strip()
    if not reason:
        rep.err(row, rid, f"{status} note has no reason before the user-action marker")
    if not action:
        rep.err(row, rid, f"{status} note has no action after the user-action marker")


def bare_script_entries(csv_path: Path) -> list[str]:
    with csv_path.open(newline="", encoding="utf-8") as fh:
        table = list(csv.reader(fh))
    seen: list[str] = []
    for cells in table[1:]:
        if len(cells) == len(HEADER) and cells[5].strip() == "pass":
            seen += [e for e in split_entries(cells[4]) if "::" not in e and e not in seen]
    return seen


def check(csv_path: Path, root: Path, results: Path | None, final: bool) -> Report:
    rep = Report(results_checked=results is not None, final=final)
    try:
        with csv_path.open(newline="", encoding="utf-8") as fh:
            table = list(csv.reader(fh))
    except OSError as e:
        rep.err(0, "-", f"cannot read csv: {e}")
        return rep
    if not table or table[0] != HEADER:
        got = ",".join(table[0]) if table else "<empty file>"
        rep.err(1, "-", f"header must be exactly {','.join(HEADER)} (got {got})")
        return rep

    evidence: Evidence | None = None
    if results is not None:
        evidence = Evidence(results, root)
        if not results.is_dir():
            rep.err(0, "-", f"results directory {results} does not exist")
        else:
            for problem in evidence.problems:
                rep.err(0, "-", problem)
            if not evidence.files_loaded:
                rep.err(0, "-", f"results directory {results} contains no recognised evidence files")

    for index, cells in enumerate(table[1:], start=2):
        if not any(c.strip() for c in cells):
            continue
        rep.total += 1
        rid = cells[0].strip() if cells else ""
        if len(cells) != len(HEADER):
            rep.err(index, rid or "-", f"expected {len(HEADER)} columns, found {len(cells)}")
            continue
        row = dict(zip(HEADER, cells))
        if not rid:
            rep.err(index, "-", "empty id")
        status = row["status"].strip()
        if status not in STATUSES:
            rep.err(index, rid or "-", f"invalid status {status!r} (allowed: {', '.join(STATUSES)})")
            continue
        rep.counts[status] += 1
        if status == "pass":
            check_pass_row(root, index, rid, row["test_path"], evidence, rep)
        elif final and status in EXCEPTION_STATUSES:
            check_exception_note(index, rid, status, row["note"], rep)

    if final:
        if rep.counts["unverified"]:
            rep.err(0, "-", f"--final: {rep.counts['unverified']} unverified row(s) remain")
        used, limit = exception_usage(rep)
        if used > limit:
            rep.err(0, "-", f"--final: {used} out-of-scope/blocked-external rows exceed the limit of {limit}")
    return rep


def exception_usage(rep: Report) -> tuple[int, int]:
    used = sum(rep.counts[s] for s in EXCEPTION_STATUSES)
    return used, int(EXCEPTION_RATIO * rep.total)


def render_text(rep: Report) -> str:
    used, limit = exception_usage(rep)
    pct = (100.0 * used / rep.total) if rep.total else 0.0
    lines = ["status             count"]
    lines += [f"{s:<17} {rep.counts[s]:>6}" for s in STATUSES]
    lines.append(f"{'total':<17} {rep.total:>6}")
    lines.append(
        f"exceptions (out-of-scope + blocked-external): {used} = {pct:.1f}% of rows; "
        f"limit {int(EXCEPTION_RATIO * 100)}% = {limit} rows"
    )
    lines.append(f"mode: static{' + results' if rep.results_checked else ''}{' + final' if rep.final else ''}")
    lines += [f"ERROR {row} {rid}: {reason}" for row, rid, reason in rep.errors]
    lines.append(f"{len(rep.errors)} error(s)")
    return "\n".join(lines)


def render_json(rep: Report) -> str:
    used, limit = exception_usage(rep)
    return json.dumps(
        {
            "total": rep.total,
            "counts": rep.counts,
            "exceptions": {"used": used, "limit": limit, "ratio": (used / rep.total) if rep.total else 0.0},
            "results_checked": rep.results_checked,
            "final": rep.final,
            "errors": [{"row": r, "id": i, "reason": why} for r, i, why in rep.errors],
            "ok": not rep.errors,
        },
        ensure_ascii=False,
        indent=2,
    )


def main(argv: list[str] | None = None) -> int:
    default_root = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--csv", type=Path, help="acceptance csv (default: docs/specs/acceptance.csv)")
    ap.add_argument("--results", type=Path, help="directory of test-run evidence written by tools/gate.sh")
    ap.add_argument("--final", action="store_true", help="enforce the final-state thresholds")
    ap.add_argument("--json", action="store_true", help="print a machine-readable report")
    ap.add_argument("--list-scripts", action="store_true", help="print bare-path entries of pass rows (gate.sh runs them)")
    ap.add_argument("--root", type=Path, default=default_root, help="repo root that test_path entries are relative to")
    args = ap.parse_args(argv)
    root = args.root.resolve()
    csv_path = args.csv if args.csv else root / "docs/specs/acceptance.csv"
    if args.list_scripts:
        for entry in bare_script_entries(csv_path):
            print(entry)
        return 0
    rep = check(csv_path, root, args.results, args.final)
    print(render_json(rep) if args.json else render_text(rep))
    return 1 if rep.errors else 0


if __name__ == "__main__":
    sys.exit(main())

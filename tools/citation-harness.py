#!/usr/bin/env python3
"""Measure the uncited-claim ratio of a research answer against the pages actually visited (A-9).

The claim and citation heuristic is the sidecar's own (agent/src/openside_agent/citations.py,
loaded by path so this runs with plain python3); see docs/specs/ultrabrowse.md §3.

    tools/citation-harness.py --answer answer.md --visited URL [--visited URL ...]
    tools/citation-harness.py --answer - --visited-file visited.json --max-uncited-ratio 0

`--visited-file` takes a JSON list of browser tool results or URLs; only their structured
`url` fields count (citations.result_urls), never URLs inside page text.
Prints a JSON report. Exit status: 0 measured (and within --max-uncited-ratio when given),
1 above the threshold, 2 unusable input.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import sys
from pathlib import Path

MODULE = Path(__file__).resolve().parents[1] / "agent/src/openside_agent/citations.py"
_spec = importlib.util.spec_from_file_location("openside_citations", MODULE)
assert _spec is not None and _spec.loader is not None
citations = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = citations
_spec.loader.exec_module(citations)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--answer", required=True, help="answer text file, or - for stdin")
    parser.add_argument("--visited", action="append", default=[], help="visited page URL")
    parser.add_argument("--visited-file", type=Path, help="JSON value whose URLs were visited")
    parser.add_argument("--max-uncited-ratio", type=float, help="fail above this ratio (0..1)")
    args = parser.parse_args(argv)

    try:
        answer = (
            sys.stdin.read() if args.answer == "-" else Path(args.answer).read_text(encoding="utf-8")
        )
        visited = set(args.visited)
        if args.visited_file is not None:
            visited |= citations.result_urls(
                json.loads(args.visited_file.read_text(encoding="utf-8"))
            )
    except (OSError, ValueError) as error:
        print(f"cannot read input: {error}", file=sys.stderr)
        return 2

    report = citations.check(answer, visited)
    print(json.dumps(report.to_json(), ensure_ascii=False))
    if args.max_uncited_ratio is not None and report.ratio > args.max_uncited_ratio:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

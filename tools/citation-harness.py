#!/usr/bin/env python3
"""Measure the uncited-claim ratio of a research answer against the pages actually visited (A-9).

The claim and citation heuristic is the sidecar's own, so this runs
`bun run agent/src/features/citations.ts --harness` (Bun from $OPENSIDE_BUN, else `bun` on PATH)
instead of keeping a second copy; see docs/specs/ultrabrowse.md §3.

    tools/citation-harness.py --answer answer.md --visited URL [--visited URL ...]
    tools/citation-harness.py --answer - --visited-file visited.json --max-uncited-ratio 0

`--visited-file` takes a JSON list of browser tool results or URLs; only their structured
`url` fields count (citations resultUrls), never URLs inside page text.
Prints a JSON report. Exit status: 0 measured (and within --max-uncited-ratio when given),
1 above the threshold, 2 unusable input.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

MODULE = Path(__file__).resolve().parents[1] / "agent/src/features/citations.ts"


def main(argv: list[str] | None = None) -> int:
    bun = os.environ.get("OPENSIDE_BUN") or "bun"
    try:
        # stdin/stdout/stderr are inherited so `--answer -` and the JSON report pass straight through.
        return subprocess.run([bun, "run", str(MODULE), "--harness", *(sys.argv[1:] if argv is None else argv)]).returncode
    except OSError as error:
        print(f"cannot run the citation checker with {bun}: {error}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())

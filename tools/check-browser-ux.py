#!/usr/bin/env python3
"""Validate browser UX decomposition coverage and every referenced evidence file."""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]
REQUIRED = {f"UX-{number:02}" for number in range(1, 18)}
STATUSES = {"implemented", "not-implemented", "needs-test", "blocked-external", "out-of-scope"}


def check(root: Path, text: str) -> list[str]:
    errors, seen = [], set()
    for line in text.splitlines():
        if not line.startswith("| UX-"):
            continue
        cells = [cell.strip() for cell in line.strip("|").split("|")]
        if len(cells) != 6:
            errors.append("UX row must have six columns")
            continue
        identifier, item, criterion, surface, status, evidence = cells
        if identifier in seen:
            errors.append(f"duplicate {identifier}")
        seen.add(identifier)
        if not item or not criterion or not surface or status not in STATUSES:
            errors.append(f"{identifier}: missing AC/surface or unknown status")
        paths = re.findall(r"`([^`]+)`", evidence)
        if status == "implemented" and not paths:
            errors.append(f"{identifier}: implemented row needs test evidence")
        for path in paths:
            file = path.split("::", 1)[0]
            target = root / file
            if not target.resolve().is_relative_to(root.resolve()) or not target.is_file():
                errors.append(f"{identifier}: evidence does not exist: {file}")
        if status in {"blocked-external", "out-of-scope"} and "User action:" not in evidence:
            errors.append(f"{identifier}: exception needs reason and User action:")
    for identifier in sorted(REQUIRED - seen):
        errors.append(f"missing {identifier}")
    for identifier in sorted(seen - REQUIRED):
        errors.append(f"unknown {identifier}; update the registry when adding an item")
    return errors


def main() -> int:
    errors = check(ROOT, (ROOT / "docs/specs/browser-ux.md").read_text())
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print(f"browser UX: {len(REQUIRED)} item ACs, evidence paths exist")
    return 0


if __name__ == "__main__":
    sys.exit(main())

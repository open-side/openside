#!/usr/bin/env python3
"""Fail when a profile's system prompt plus tool definitions exceed the token budget (A-2).

Measures `open.system_prompt_parts` + `open.tools` exactly as the daemon sends them to the
sidecar. The payloads are docs/specs/profiles/<profile>.open.json, which the opensided test
`profile_*_open_payload_match_snapshot` keeps byte-identical to the real `open` message.

No tokenizer runs here (the gate needs no network or model files). ASCII text counts as
ceil(bytes / BYTES_PER_TOKEN), calibrated once against real BPE tokenizers so that it
over-counts every one of them on these payloads; every non-ASCII UTF-8 byte counts as a
whole token, the worst case of byte-level BPE (Hangul often takes 2-3 tokens per
syllable). Method and numbers are in docs/specs/profiles.md. Only the narrowed profiles
have committed payloads; A-2 budgets the browser-agent default toolset, not `full`.
Exit status: 0 within budget, 1 over budget, 2 unusable input. Python 3 stdlib only.
"""

from __future__ import annotations

import argparse
import json
import math
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PROFILE_DIR = ROOT / "docs/specs/profiles"
DEFAULT_PROFILES = ("browser-agent", "ultrabrowse")
DEFAULT_LIMIT = 10_000
# Lowest bytes/token observed during calibration was 4.19 (Claude tokenizer on tool JSON
# schemas); 3.0 over-counts that by about 40% so denser future text stays over-counted.
BYTES_PER_TOKEN = 3.0


def system_text(payload: dict) -> str:
    parts = payload["system_prompt_parts"]
    if not isinstance(parts, list) or not all(isinstance(part, str) for part in parts):
        raise ValueError("system_prompt_parts must be a list of strings")
    return "\n\n".join(parts)


def tools_text(payload: dict) -> str:
    tools = payload["tools"]
    if not isinstance(tools, list):
        raise ValueError("tools must be a list")
    return "\n".join(
        json.dumps(tool, ensure_ascii=False, separators=(",", ":"), sort_keys=True)
        for tool in tools
    )


def estimate(text: str) -> int:
    ascii_chars = sum(1 for char in text if char.isascii())
    other_bytes = len(text.encode("utf-8")) - ascii_chars
    return math.ceil(ascii_chars / BYTES_PER_TOKEN) + other_bytes


def measure(payload: dict) -> dict[str, int]:
    system = estimate(system_text(payload))
    tools = estimate(tools_text(payload))
    return {"system": system, "tools": tools, "total": system + tools}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--profile",
        action="append",
        help=f"profile name under {PROFILE_DIR.relative_to(ROOT)} (default: {', '.join(DEFAULT_PROFILES)})",
    )
    parser.add_argument("--payload", action="append", type=Path, help="explicit open payload JSON file")
    parser.add_argument("--limit", type=int, default=DEFAULT_LIMIT)
    parser.add_argument("--json", action="store_true", help="print machine-readable results")
    args = parser.parse_args(argv)

    targets: list[tuple[str, Path]] = [
        (path.name.removesuffix(".json").removesuffix(".open"), path) for path in args.payload or []
    ]
    if not args.payload or args.profile:
        targets += [
            (name, PROFILE_DIR / f"{name}.open.json") for name in args.profile or DEFAULT_PROFILES
        ]

    results = {}
    for name, path in targets:
        try:
            results[name] = measure(json.loads(path.read_text(encoding="utf-8")))
        except (OSError, ValueError, KeyError, TypeError) as error:
            print(f"{name}: cannot measure {path}: {error}", file=sys.stderr)
            return 2

    over = [name for name, result in results.items() if result["total"] > args.limit]
    if args.json:
        print(json.dumps({"limit": args.limit, "bytes_per_token": BYTES_PER_TOKEN, "profiles": results}))
    else:
        for name, result in results.items():
            verdict = "OVER BUDGET" if name in over else "ok"
            print(
                f"{name}: {result['total']} estimated tokens "
                f"(system {result['system']}, tools {result['tools']}; limit {args.limit}) {verdict}"
            )
    return 1 if over else 0


if __name__ == "__main__":
    sys.exit(main())

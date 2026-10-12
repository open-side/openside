#!/usr/bin/env python3
"""Audit implementation language boundaries without treating build outputs as source."""
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
LANGUAGES = {
    ".rs":"Rust", ".py":"Python", ".js":"JavaScript", ".mjs":"JavaScript", ".cjs":"JavaScript", ".jsx":"JavaScript",
    ".ts":"TypeScript", ".tsx":"TypeScript", ".c":"C", ".cc":"C++", ".cpp":"C++", ".cxx":"C++", ".h":"C/C++",
    ".hpp":"C++", ".go":"Go", ".java":"Java", ".rb":"Ruby", ".swift":"Swift", ".kt":"Kotlin", ".cs":"C#",
    ".php":"PHP", ".lua":"Lua", ".zig":"Zig", ".dart":"Dart", ".scala":"Scala", ".sh":"Shell",
}
# This is JS evaluated inside the Rust-owned QuickJS sandbox, not a controller.
EMBEDDED = {"crates/openside-repl/src/prelude.js":"sandboxed browser-repl guest API"}
UI_JS_ASSETS = {"web/eslint.config.js", "extension/eslint.config.js", "web/public/sw.js", "web/public/theme-boot.js"}


def audit(root: Path) -> tuple[list[str], list[str]]:
    report, errors = [], []
    components = [(path, "Rust") for path in sorted((root / "crates").iterdir()) if path.is_dir()]
    components += [(root / "agent", "Python"), (root / "web", "TypeScript"), (root / "extension", "TypeScript")]
    for component, expected in components:
        counts: dict[str, int] = {}
        for path in component.rglob("*"):
            relative = path.relative_to(root).as_posix()
            parts = path.relative_to(component).parts
            if not path.is_file() or any(part in {"node_modules", ".venv", "target", "dist", "__pycache__", ".pytest_cache"} for part in parts):
                continue
            language = LANGUAGES.get(path.suffix)
            if language is None:
                continue
            counts[language] = counts.get(language, 0) + 1
            if relative in EMBEDDED:
                continue
            # JavaScript build configs/service-worker assets do not move UI logic out of TS.
            if expected == "TypeScript" and relative in UI_JS_ASSETS:
                continue
            if language != expected:
                errors.append(f"{relative}: {language} violates {expected} component boundary")
        report.append(f"{component.relative_to(root)}: " + ", ".join(f"{name}={count}" for name, count in sorted(counts.items())))
        if not counts.get(expected):
            errors.append(f"{component.relative_to(root)}: no {expected} implementation")
    # TypeScript is exclusive to the two UI components, even in future top-level code.
    for path in root.rglob("*"):
        if path.suffix not in {".ts", ".tsx"} or not path.is_file():
            continue
        relative = path.relative_to(root)
        if any(part in {"node_modules", ".venv", "target", "dist", ".git"} for part in relative.parts):
            continue
        if relative.parts[0] not in {"web", "extension"}:
            errors.append(f"{relative}: TypeScript outside web/extension")
    return report, errors


def main() -> int:
    report, errors = audit(ROOT)
    print("\n".join(report))
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("language boundaries: pass")
    return 0


if __name__ == "__main__":
    sys.exit(main())

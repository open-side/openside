#!/usr/bin/env python3
"""Check Rust/npm/Python dependency licenses and deterministically generate NOTICE."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tomllib

ROOT = Path(__file__).resolve().parents[1]
# BlueOak is permissive; Python and CC-BY cover argparse and build-time browser data.
NPM_ALLOW = {
    "MIT", "MIT-0", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause", "ISC",
    "0BSD", "Unlicense", "CC0-1.0", "BlueOak-1.0.0", "Python-2.0", "CC-BY-4.0",
}
PYTHON_ALLOW = NPM_ALLOW | {"PSF-2.0", "CNRI-Python"}
# MPL-2.0 is file-level copyleft; these are used unmodified (certifi at runtime, hypothesis in tests only).
PYTHON_MPL = {"certifi", "hypothesis"}
# Pre-PEP 639 free-text `License:` values, mapped after reading the licence text each package ships.
PYTHON_LICENSE_TEXT = {"Apache 2.0": "Apache-2.0", "Apache License, Version 2.0": "Apache-2.0", "MIT License": "MIT", "MIT license": "MIT"}
# Metadata says only "BSD"; the clause count comes from the shipped licence text.
PYTHON_REVIEWED = {"nodeenv": "BSD-3-Clause", "pyasn1-modules": "BSD-2-Clause"}
# Locked behind platform markers (win32, emscripten), so a Linux sync never installs them; licence from PyPI metadata.
PYTHON_OTHER_PLATFORM = {"colorama": "BSD-3-Clause", "httpx2-jsfetch": "BSD-3-Clause"}


# Copied into the repository rather than installed from a registry: (what, source, license, text).
# The design system is adapted from aside-relay; nothing is taken from the Aside product itself.
VENDORED = [
    ("aside-relay: web design system and UI patterns; web/src/theme-presets.ts holds its palette color values for 29 theme families (names identify the palettes; no desktop application code is bundled)", "https://github.com/mym0404/aside-relay", "MIT", "web/third_party/aside-relay.LICENSE"),
    ("aside-remote: upstream of aside-relay", "https://github.com/esc5221/aside-remote", "MIT", "web/third_party/aside-remote.LICENSE"),
    ("Wanted Sans Variable 1.0.3 font (web/src/assets/fonts/wanted-sans)", "https://github.com/wanteddev/wanted-sans", "OFL-1.1", "web/src/assets/fonts/wanted-sans/OFL.txt"),
    ("D2Coding 1.3.2 font (web/src/assets/fonts/d2coding)", "https://github.com/naver/d2codingfont", "OFL-1.1", "web/src/assets/fonts/d2coding/OFL.txt"),
    ("aside-benchmarks result schema: docs/specs/bench-schema.json is an independent reconstruction of its public TypeScript result types (bu-bench-v1/runner/types.ts @7cde728); no code is copied", "https://github.com/at-inc/aside-benchmarks", "MIT", "bench/third_party/aside-benchmarks.LICENSE"),
]

# Read for design only; no code or text is copied, so there is no licence text to reproduce.
DESIGN_REFERENCES = [
    ("xplorer agent gateway (focus arbiter, tab ownership); see browser/NOTICE.md", "https://github.com/daniel-farina/xplorer"),
]


def allowed_expression(expression: str, allowed: set[str]) -> bool:
    tokens = re.findall(r"[A-Za-z0-9.+-]+|[()]", expression)
    if " ".join(tokens).replace(" ", "") != expression.replace(" ", ""):
        return False
    position = 0

    def atom() -> bool:
        nonlocal position
        if position >= len(tokens):
            raise ValueError("missing license")
        token = tokens[position]
        position += 1
        if token == "(":
            result = either()
            if position >= len(tokens) or tokens[position] != ")":
                raise ValueError("unclosed expression")
            position += 1
            return result
        if token in {"AND", "OR", ")"}:
            raise ValueError("unexpected operator")
        return token in allowed

    def both() -> bool:
        nonlocal position
        result = atom()
        while position < len(tokens) and tokens[position] == "AND":
            position += 1
            right = atom()
            result = result and right
        return result

    def either() -> bool:
        nonlocal position
        result = both()
        while position < len(tokens) and tokens[position] == "OR":
            position += 1
            right = both()
            result = result or right
        return result

    try:
        result = either()
        return result and position == len(tokens)
    except ValueError:
        return False


def license_files(directory: Path) -> list[tuple[str, str]]:
    found = []
    for path in sorted(directory.iterdir()):
        name = path.name.upper()
        if path.is_file() and (name.startswith(("LICENSE", "LICENCE", "COPYING")) or name == "NOTICE"):
            found.append((path.name, path.read_text(errors="replace").strip()))
    return found


def npm_inventory(root: Path) -> list[dict]:
    inventory = []
    for component in ("web", "extension"):
        command = ["pnpm", "-C", str(root / component)]
        # pnpm licenses defaults to the configured store, which can differ from this install.
        modules = root / component / "node_modules/.modules.yaml"
        if modules.is_file():
            for line in modules.read_text().splitlines():
                if line.startswith("storeDir:"):
                    store = line.split(":", 1)[1].strip()
                    if store.startswith('"'):
                        store = json.loads(store)
                    elif store.startswith("'") and store.endswith("'"):
                        store = store[1:-1].replace("''", "'")
                    command.append(f"--config.store-dir={store}")
                    break
        command += ["licenses", "list", "--json"]
        data = json.loads(subprocess.check_output(command, text=True))
        if not data or not any(data.values()):
            raise ValueError(f"{component}: empty license inventory; run pnpm install --frozen-lockfile")
        for expression, packages in data.items():
            if not allowed_expression(expression, NPM_ALLOW):
                raise ValueError(f"{component}: prohibited or unknown license {expression}")
            for package in packages:
                texts: dict[str, tuple[str, str]] = {}
                for directory in package["paths"]:
                    for name, text in license_files(Path(directory)):
                        texts[hashlib.sha256(text.encode()).hexdigest()] = (name, text)
                inventory.append({"component":component,"name":package["name"],"versions":sorted(package["versions"]),"license":expression,"texts":sorted(texts.values())})
    return sorted(inventory, key=lambda item: (item["component"], item["name"], item["versions"]))


def python_name(name: str) -> str:
    return re.sub(r"[-_.]+", "-", name).lower()


def python_license(name: str, metadata: str) -> str:
    if name in PYTHON_REVIEWED:
        return PYTHON_REVIEWED[name]
    for field in ("License-Expression", "License"):
        match = re.search(rf"^{field}: (.+)$", metadata, re.M)
        if match:
            value = match.group(1).strip()
            return PYTHON_LICENSE_TEXT.get(value, value)
    raise ValueError(f"python {name}: no license metadata; review it and add it to PYTHON_REVIEWED")


def python_inventory(root: Path) -> list[dict]:
    inventory = []
    for component in ("agent", "bench"):
        lock = tomllib.loads((root / component / "uv.lock").read_text())
        installed: dict[str, Path] = {}
        for dist in sorted((root / component / ".venv").glob("lib/python*/site-packages/*.dist-info")):
            header = (dist / "METADATA").read_text(errors="replace").split("\n\n", 1)[0]
            installed[python_name(re.search(r"^Name: (.+)$", header, re.M).group(1).strip())] = dist
        if not installed:
            raise ValueError(f"{component}: no installed Python environment; run uv sync --frozen --project {component}")
        for package in lock["package"]:
            name, version = package["name"], package["version"]
            if "registry" not in package.get("source", {}):
                continue  # the project itself (editable or virtual)
            texts: list[tuple[str, str]] = []
            dist = installed.get(name)
            if dist is None:
                if name not in PYTHON_OTHER_PLATFORM:
                    raise ValueError(f"{component}: {name} {version} is locked but not installed; run uv sync --frozen --project {component}")
                expression = PYTHON_OTHER_PLATFORM[name]
            else:
                header = (dist / "METADATA").read_text(errors="replace").split("\n\n", 1)[0]
                installed_version = re.search(r"^Version: (.+)$", header, re.M).group(1).strip()
                if installed_version != version:
                    raise ValueError(f"{component}: {name} installed {installed_version}, locked {version}; run uv sync --frozen --project {component}")
                expression = python_license(name, header)
                # PEP 639 wheels keep texts under licenses/ (often without License-File headers); older ones at the root.
                files = sorted(path for path in (dist / "licenses").rglob("*") if path.is_file())
                texts = [(path.relative_to(dist / "licenses").as_posix(), path.read_text(errors="replace").strip()) for path in files]
                texts += license_files(dist)
            allowed = PYTHON_ALLOW | ({"MPL-2.0"} if name in PYTHON_MPL else set())
            if not allowed_expression(expression, allowed):
                raise ValueError(f"{component}: python {name} {version} has prohibited or unknown license {expression}")
            inventory.append({"component": component, "name": name, "version": version, "license": expression,
                              "installed": dist is not None, "texts": texts})
    return sorted(inventory, key=lambda item: (item["component"], item["name"]))


def vendored_inventory(root: Path) -> list[tuple[str, str, str, str]]:
    entries = []
    for what, source, license_id, text in VENDORED:
        path = root / text
        if not path.is_file():
            raise ValueError(f"vendored notice missing: {text}")
        entries.append((what, source, license_id, path.read_text(errors="replace").strip()))
    return entries


def notice(root: Path) -> str:
    metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--format-version", "1"], cwd=root, text=True))
    rust = sorted((p for p in metadata["packages"] if p["source"] is not None), key=lambda p: (p["name"], p["version"]))
    chunks = ["openside third-party notices", "Generated by tools/check-licenses.py --write; do not edit.",
              "Rust inventory covers Cargo.lock across platforms; npm inventory covers installed web/extension dependencies, including development tools; Python inventory covers agent/ and bench/ uv.lock, including development tools.",
              "Browser Chromium overlay notices are maintained separately in browser/NOTICE.md.",
              "License expressions retain upstream choices; r-efi is used under MIT, not LGPL. option-ext, certifi and hypothesis retain MPL-2.0 file-level obligations; they are used unmodified and their source is available from crates.io and PyPI."]
    for package in rust:
        chunks.append(f"\nRust: {package['name']} {package['version']}\nLicense: {package['license'] or 'SEE LICENSE FILE'}")
        files = license_files(Path(package["manifest_path"]).parent)
        if not files and package.get("license_file"):
            path = Path(package["manifest_path"]).parent / package["license_file"]
            files = [(path.name, path.read_text(errors="replace").strip())]
        chunks.extend(f"--- {name} ---\n{text}" for name, text in files)
    for package in npm_inventory(root):
        chunks.append(f"\nnpm ({package['component']}): {package['name']} {', '.join(package['versions'])}\nLicense: {package['license']}")
        chunks.extend(f"--- {name} ---\n{text}" for name, text in package["texts"])
    for package in python_inventory(root):
        chunks.append(f"\nPython ({package['component']}): {package['name']} {package['version']}\nLicense: {package['license']}")
        if not package["installed"]:
            chunks.append("Locked for other platforms only; licence from upstream package metadata.")
        chunks.extend(f"--- {name} ---\n{text}" for name, text in package["texts"])
    for what, source, license_id, text in vendored_inventory(root):
        chunks.append(f"\nVendored: {what}\nSource: {source}\nLicense: {license_id}")
        chunks.append(f"--- LICENSE ---\n{text}")
    for what, source in DESIGN_REFERENCES:
        chunks.append(f"\nDesign reference (no code copied): {what}\nSource: {source}")
    return "\n\n".join(chunks) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--npm-only", action="store_true")
    args = parser.parse_args()
    try:
        if args.npm_only:
            packages = npm_inventory(ROOT)
            print(f"npm licenses: {len(packages)} package records accepted; {len(vendored_inventory(ROOT))} vendored notices present")
            return 0
        deny = os.environ.get("CARGO_DENY", "cargo-deny")
        subprocess.run([deny, "--log-level", "error", "check"], cwd=ROOT, check=True)
        tomllib.loads((ROOT / "deny.toml").read_text())
        expected = notice(ROOT)
        path = ROOT / "NOTICE"
        if args.write:
            path.write_text(expected)
            print("NOTICE generated")
        elif not path.is_file() or path.read_text() != expected:
            print("NOTICE is stale: run tools/check-licenses.py --write", file=sys.stderr)
            return 1
        else:
            print("Rust/npm/Python licenses and NOTICE: pass")
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"license check failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError):
            for detail in (error.stdout, error.stderr):
                if detail:
                    print(detail.decode(errors="replace") if isinstance(detail, bytes) else detail, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())

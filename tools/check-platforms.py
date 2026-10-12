#!/usr/bin/env python3
"""Compile the workspace with installed target std or an installed nightly's rust-src.

The MSVC target needs the Microsoft CRT and SDK headers for the C dependencies (ring, sqlite,
quickjs). When `cargo-xwin` is on PATH, or `OPENSIDE_XWIN` points at the executable (or at an
install root holding `bin/cargo-xwin`), that target is checked through `cargo xwin check`.
"""
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGETS = ("x86_64-apple-darwin", "aarch64-apple-darwin", "x86_64-pc-windows-msvc",
           "x86_64-pc-windows-gnu")
TARGET_DIR = Path(tempfile.gettempdir()) / "openside-xcheck"
XWIN_TARGET = "x86_64-pc-windows-msvc"
NATIVE_ACTION = "run cargo check --workspace --all-targets --locked -j 8 on the matching CI/native runner."


def find_build_std_toolchain(output=subprocess.check_output) -> str | None:
    try:
        toolchains = output(["rustup", "toolchain", "list"], text=True).splitlines()
    except (OSError, subprocess.CalledProcessError):
        return None
    for toolchain in sorted((line.split()[0] for line in toolchains if line.startswith("nightly")), reverse=True):
        try:
            sysroot = output(["rustup", "run", toolchain, "rustc", "--print", "sysroot"], text=True).strip()
        except (OSError, subprocess.CalledProcessError):
            continue
        if (Path(sysroot) / "lib/rustlib/src/rust/library/Cargo.toml").is_file():
            return toolchain
    return None


def find_xwin(env=os.environ, which=shutil.which) -> Path | None:
    configured = env.get("OPENSIDE_XWIN")
    if configured:
        path = Path(configured)
        if path.is_dir():
            path = path / "bin" / "cargo-xwin"
        return path if path.is_file() else None
    found = which("cargo-xwin")
    return Path(found) if found else None


def external_blocker(output: str) -> bool:
    # A Rust diagnostic always fails the gate, even if a sibling C build also lacks an SDK.
    if re.search(r"error\[E\d+\]", output):
        return False
    markers = (
        "fatal error: 'TargetConditionals.h' file not found",
        "fatal error: 'stdio.h' file not found",
        "fatal error: 'stdlib.h' file not found",
        "fatal error: 'windows.h' file not found",
        "fatal error: 'pthread.h' file not found",
        "fatal error: 'sys/types.h' file not found",
        "unrecognized command-line option '-arch'",
        "unrecognized command-line option '-mmacosx-version-min=",
        'failed to find tool "lib.exe"',
        'failed to find tool "x86_64-w64-mingw32-gcc"',
        "error calling dlltool",
        "failed to find tool",
        "unable to find library -lSystem",
    )
    return any(marker in output for marker in markers)


def check(installed: set[str], run=subprocess.run, *, build_std_toolchain: str | None = None,
          xwin: Path | None = None) -> tuple[list[str], int]:
    unavailable, failures = [], 0
    for target in TARGETS:
        command = ["cargo"]
        use_xwin = xwin is not None and target == XWIN_TARGET
        if target not in installed:
            if build_std_toolchain is None:
                unavailable.append(target)
                print(f"BLOCKED-EXTERNAL {target}: target std and installed nightly rust-src unavailable; User action: {NATIVE_ACTION}")
                continue
            command += [f"+{build_std_toolchain}"]
        command += ["xwin"] if use_xwin else []
        command += ["check", "--workspace", "--all-targets", "--locked", "-j", "8", "--target", target]
        if target not in installed:
            command += ["-Zbuild-std=std,panic_abort", "--target-dir", str(TARGET_DIR)]
        print(f"platform compile command: {' '.join(command)}", flush=True)
        options = {}
        if use_xwin:
            # cargo finds `cargo xwin` as a `cargo-xwin` executable on PATH.
            options["env"] = {**os.environ, "PATH": f"{xwin.parent}{os.pathsep}{os.environ.get('PATH', '')}"}
        result = run(command, cwd=ROOT, check=False, text=True, capture_output=True, **options)
        diagnostics = (getattr(result, "stdout", "") or "") + (getattr(result, "stderr", "") or "")
        if diagnostics:
            print(diagnostics, end="" if diagnostics.endswith("\n") else "\n")
        if result.returncode == 0:
            print(f"PASS {target}: all workspace crates and targets compile-checked.")
        elif external_blocker(diagnostics):
            unavailable.append(target)
            print(f"BLOCKED-EXTERNAL {target}: cross compiler/SDK unavailable; exact compiler output above. User action: configure target CC/AR and SDK headers, or {NATIVE_ACTION}")
        else:
            failures += 1
            print(f"FAIL {target}: workspace compile check exited {result.returncode}.")
    return unavailable, failures


def main() -> int:
    installed = set(subprocess.check_output(["rustup", "target", "list", "--installed"], text=True).split())
    xwin = find_xwin()
    if xwin is None and os.environ.get("OPENSIDE_XWIN"):
        print(f"OPENSIDE_XWIN={os.environ['OPENSIDE_XWIN']} is not a cargo-xwin executable or install root; {XWIN_TARGET} stays BLOCKED-EXTERNAL.")
    unavailable, failures = check(installed, build_std_toolchain=find_build_std_toolchain(), xwin=xwin)
    print(f"platform compile checks: failures={failures}, unavailable_targets={len(unavailable)}")
    return int(bool(failures))


if __name__ == "__main__":
    sys.exit(main())

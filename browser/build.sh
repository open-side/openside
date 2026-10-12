#!/usr/bin/env bash
# Applies the overlay and incrementally builds `chrome` under the resource guard.
# Usage: build.sh [SRC_DIR]   (default: $OPENSIDE_BROWSER_BUILD_DIR/portablelinux/build/src)
set -euo pipefail

if [[ $# -gt 1 ]]; then
    printf 'Usage: %s [SRC_DIR]\n' "$0" >&2
    exit 2
fi
OVERLAY_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
if [[ $# -eq 1 ]]; then
    SRC_DIR="$1"
else
    : "${OPENSIDE_BROWSER_BUILD_DIR:?set OPENSIDE_BROWSER_BUILD_DIR or pass SRC_DIR}"
    SRC_DIR="${OPENSIDE_BROWSER_BUILD_DIR}/portablelinux/build/src"
fi
SRC_DIR="$(cd -- "$SRC_DIR" && pwd -P)"
args_file="${SRC_DIR}/out/Default/args.gn"
[[ -f "${SRC_DIR}/out/Default/build.ninja" && -f "$args_file" ]] || {
    printf 'Generate the pinned base GN build before building the overlay.\n' >&2
    exit 1
}

"${OVERLAY_DIR}/apply.sh" "$SRC_DIR"
marker='# openside overrides (appended after ungoogled flags)'
if ! grep -qxF "$marker" "$args_file"; then
    printf '\n%s\n' "$marker" >>"$args_file"
    cat "${OVERLAY_DIR}/args.gn" >>"$args_file"
fi

OPENSIDE_CHROMIUM_SRC="$SRC_DIR" exec "${OVERLAY_DIR}/guarded-build.sh" chrome

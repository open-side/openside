#!/usr/bin/env bash
# Rebuild web/dist and capture the browser surfaces of the web UI headless (live preview,
# browser sheet, takeover, code requests) at 390x844 and 1440x900, light and dark. See
# crates/openside-e2e/tests/web_screens_browser.rs.
#
#   tools/qa/web-screens-browser.sh [out-dir]
#   OPENSIDE_SCREENS_ONLY=otp-sheet,otp-dock-settings tools/qa/web-screens-browser.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
export OPENSIDE_SCREENS_DIR="${1:-${OPENSIDE_SCREENS_DIR:-/tmp/openside-qa/redesign/browser}}"
if [[ "${OPENSIDE_SCREENS_SKIP_BUILD:-}" != 1 ]]; then
  pnpm -C "$root/web" build
fi
cd "$root"
capture() { cargo test -p openside-e2e --test web_screens_browser -- --ignored --nocapture; }
capture
# A takeover admits one remote-screen connection: the full run above opened it in its first
# pass, so each other pass gets a fresh run of its own.
only="${OPENSIDE_SCREENS_ONLY:-takeover,takeover-pip}"
if [[ ",$only," == *,takeover,* || ",$only," == *,takeover-pip,* ]]; then
  for pass in light-desktop dark-phone dark-desktop; do
    OPENSIDE_SCREENS_ONLY=takeover,takeover-pip OPENSIDE_SCREENS_PASSES="$pass" capture
  done
fi

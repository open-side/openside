#!/usr/bin/env bash
# Rebuild web/dist and capture the web UI screens headless (390x844 and 1440x900, light and
# dark) from a hermetic daemon on a scripted model. See crates/openside-e2e/tests/web_screens.rs.
#
#   tools/qa/web-screens.sh [out-dir]
#   OPENSIDE_SCREENS_ROUTES=/skills,/settings tools/qa/web-screens.sh /tmp/openside-qa/redesign/pages
#   OPENSIDE_SCREENS_ONLY=conversation,queue-menu tools/qa/web-screens.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
export OPENSIDE_SCREENS_DIR="${1:-${OPENSIDE_SCREENS_DIR:-/tmp/openside-qa/redesign/core}}"
if [[ "${OPENSIDE_SCREENS_SKIP_BUILD:-}" != 1 ]]; then
  pnpm -C "$root/web" build
fi
cd "$root"
cargo test -p openside-e2e --test web_screens -- --ignored --nocapture

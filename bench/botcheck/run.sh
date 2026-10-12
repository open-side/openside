#!/usr/bin/env bash
set -euo pipefail
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
cd "$ROOT"
exec cargo run --offline --locked -j 8 -p openside-browser --example botcheck -- "$@"

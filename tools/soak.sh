#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
export OPENSIDE_SOAK_SECONDS=${OPENSIDE_SOAK_SECONDS:-900}
export OPENSIDE_SOAK_SESSIONS=${OPENSIDE_SOAK_SESSIONS:-10}
export OPENSIDE_SOAK_OUTPUT=${OPENSIDE_SOAK_OUTPUT:-target/soak-summary.json}
case $OPENSIDE_SOAK_OUTPUT in
  /*) ;;
  *) export OPENSIDE_SOAK_OUTPUT="$PWD/$OPENSIDE_SOAK_OUTPUT" ;;
esac
cargo test -j 8 -p openside-e2e --test soak soak -- --ignored --exact --nocapture

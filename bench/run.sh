#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

# Dependency installation is a separate, explicit networked prerequisite.
export UV_OFFLINE=1
export UV_PYTHON_DOWNLOADS=never
export UV_CACHE_DIR="${UV_CACHE_DIR:-$ROOT/target/uv-cache}"
if [[ -z "${OPENSIDE_BENCH_DAEMON:-}" ]]; then
    cargo build --offline --locked -j 8 -p opensided --bin opensided
    TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
    if [[ "$TARGET" != /* ]]; then TARGET="$ROOT/$TARGET"; fi
    OPENSIDE_BENCH_DAEMON="$TARGET/debug/opensided"
fi
if [[ ! -x "$OPENSIDE_BENCH_DAEMON" ]]; then
    printf 'Benchmark daemon is not executable: %s\n' "$OPENSIDE_BENCH_DAEMON" >&2
    exit 2
fi

PYTHON="${OPENSIDE_BENCH_PYTHON:-}"
if [[ -z "$PYTHON" ]]; then
    PYTHON=$(uv run --offline --frozen --project agent python -c 'import sys; print(sys.executable)')
fi
exec uv run --offline --frozen --project bench python bench/runner.py \
    --daemon "$OPENSIDE_BENCH_DAEMON" --python "$PYTHON" "$@"

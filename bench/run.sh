#!/usr/bin/env bash
set -euo pipefail

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$ROOT"

# Dependency installation is a separate, explicit networked prerequisite.
export UV_OFFLINE=1
export UV_PYTHON_DOWNLOADS=never
export UV_CACHE_DIR="${UV_CACHE_DIR:-$ROOT/target/uv-cache}"
if [[ -z "${OPENSIDE_BENCH_DAEMON:-}" ]]; then
    cargo build --offline --locked -j "${CARGO_BUILD_JOBS:-4}" -p opensided --bin opensided
    TARGET="${CARGO_TARGET_DIR:-$ROOT/target}"
    if [[ "$TARGET" != /* ]]; then TARGET="$ROOT/$TARGET"; fi
    OPENSIDE_BENCH_DAEMON="$TARGET/debug/opensided"
fi
if [[ ! -x "$OPENSIDE_BENCH_DAEMON" ]]; then
    printf 'Benchmark daemon is not executable: %s\n' "$OPENSIDE_BENCH_DAEMON" >&2
    exit 2
fi

BUN="${OPENSIDE_BUN:-$(command -v bun || true)}"
if [[ -z "$BUN" ]]; then
    printf 'Bun is required for the sidecar: put bun on PATH or set OPENSIDE_BUN\n' >&2
    exit 2
fi
exec uv run --offline --frozen --project bench python bench/runner.py \
    --daemon "$OPENSIDE_BENCH_DAEMON" --bun "$BUN" "$@"

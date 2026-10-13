#!/usr/bin/env bash
# Full local gate. Evidence for tools/check-acceptance.py lands in $OPENSIDE_GATE_DIR.
# Every step prints `=== <step> rc=<n>`; the script keeps going after a failure and exits non-zero
# if any step failed. Set OPENSIDE_GATE_FINAL=1 to enforce the final acceptance thresholds.
set -uo pipefail
cd "$(dirname "$0")/.."
ROOT=$PWD
DIR=${OPENSIDE_GATE_DIR:-target/gate}
case $DIR in /*) ;; *) DIR=$ROOT/$DIR ;; esac
JOBS=${OPENSIDE_GATE_JOBS:-${CARGO_BUILD_JOBS:-4}}

rm -rf "$DIR"
mkdir -p "$DIR"
: >"$DIR/scripts.txt"

FAILED=0
step() {
  local name=$1 rc
  shift
  echo "--- $name"
  "$@"
  rc=$?
  echo "=== $name rc=$rc"
  [ "$rc" -eq 0 ] || FAILED=1
}

cargo_test() { cargo test -j "$JOBS" --no-fail-fast 2>&1 | tee "$DIR/cargo-test.log"; }
cargo_clippy() { cargo clippy --all-targets -j "$JOBS" -- -D warnings; }


responsive_checks() {
  python3 tools/qa/responsive.py --results "$DIR" --jobs "$JOBS"
}

mobile_remote_checks() {
  python3 tools/qa/mobile_remote.py --results "$DIR" --jobs "$JOBS"
}

browser_ux_checks() {
  tools/check-browser-ux.py &&
    OPENSIDE_QA_OUT="$DIR" tools/qa/headed-ux.sh sway stock --prewarm-only \
      --daemon-bin "$ROOT/target/debug/e2e-opensided" --junitxml "$DIR/headed-pytest.xml"
}
agent_checks() {
  (cd agent && uv run ruff check && uv run ruff format --check && uv run pyright \
    && uv run pytest -m 'not live' --junitxml="$DIR/pytest.xml")
}

has_script() { (cd "$1" && node -e 'process.exit(require("./package.json").scripts?.[process.argv[1]] ? 0 : 1)' "$2"); }

# Runs `pnpm run <script>` when the package defines it; a missing script is a visible SKIP, not a failure.
pnpm_script() {
  local dir=$1 script=$2
  shift 2
  if has_script "$dir" "$script"; then
    step "$dir $script" bash -c 'cd "$1" && shift && pnpm run "$@"' _ "$dir" "$script" "$@"
  else
    echo "SKIP $dir $script: not defined"
  fi
}

js_checks() {
  local dir=$1
  [ -f "$dir/package.json" ] || return 0
  # Reuse the store node_modules was installed from; a different store makes pnpm refuse or rebuild the tree
  # and leaves `pnpm licenses` (tools/check-licenses.py) reading the wrong store.
  local store=""
  if [ -z "${npm_config_store_dir:-}" ] && [ -f "$dir/node_modules/.modules.yaml" ]; then
    store=$(sed -n 's/^storeDir: *//p' "$dir/node_modules/.modules.yaml" | head -n 1)
    [ -d "$store" ] || store=""
  fi
  step "$dir install" bash -c 'cd "$1" && pnpm install --frozen-lockfile ${2:+--config.store-dir="$2"}' _ "$dir" "$store"
  pnpm_script "$dir" lint
  pnpm_script "$dir" typecheck
  # pnpm forwards args after the script name itself; an explicit `--` would reach vitest as a file filter.
  pnpm_script "$dir" test --reporter=default --reporter=json --outputFile="$DIR/$dir.vitest.json"
  pnpm_script "$dir" build
}

acceptance_scripts() {
  local rc=0 path
  while IFS= read -r path; do
    [ -n "$path" ] || continue
    if [ -x "$path" ] && "$path"; then
      echo "PASS $path" >>"$DIR/scripts.txt"
    else
      echo "FAIL $path" >>"$DIR/scripts.txt"
      rc=1
    fi
  done < <(tools/check-acceptance.py --list-scripts)
  return $rc
}

step "cargo fmt" cargo fmt --check
step "cargo clippy" cargo_clippy
# Browser e2e tests in `cargo test` serve web/dist and load the built extension, so build them first.
js_checks web
js_checks extension
step "cargo test" cargo_test
step "responsive first browser action" responsive_checks
step "mobile encrypted relay journey twice" mobile_remote_checks
step "agent (ruff, pyright, pytest)" agent_checks
step "portability" tools/check-portability.sh
step "tools tests" uv run --no-project --with pytest pytest -q tools/tests --junitxml="$DIR/tools-pytest.xml"
step "bench dependencies" uv sync --frozen --project bench
step "bench tests" uv run --offline --frozen --project bench pytest bench/tests -m 'not live' --junitxml="$DIR/bench-pytest.xml"
step "CI workflow" tools/check-ci.py
step "component languages" tools/check-languages.py
step "Rust/npm/Python licenses and NOTICE" tools/check-licenses.py
step "browser UX evidence" browser_ux_checks
step "installed platform targets" tools/check-platforms.py
step "openapi breaking changes" tools/openapi-diff.py
# Payloads are kept current by the opensided profile snapshot tests in the cargo test step.
step "prompt token budget" tools/token-budget.py
step "generated docs drift" tools/gen-docs.sh --check
step "acceptance scripts" acceptance_scripts
# shellcheck disable=SC2086
step "acceptance" tools/check-acceptance.py --results "$DIR" ${OPENSIDE_GATE_FINAL:+--final}

echo "GATE rc=$FAILED"
exit "$FAILED"

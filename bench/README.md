# Local browser task benchmark

Run a deterministic suite through the **real opensided daemon, Python sidecar,
and Chromium**. There are five loopback-only tasks: form fill, three-page
navigation, table extraction, fake login with an automatically resolved 2FA
information ticket, and a file download. The grader checks fixture state and
actual downloaded bytes, not the model's claim that it succeeded.
The download script waits for the download event and calls `saveAs("report.csv")`
before grading. Chromium's internal GUID-named file alone is not a completed
named-file task.

These are synthetic regression tasks, **not** the public Online-Mind2Web,
BU Bench V1, or Odysseys datasets. The output uses the public BU Bench V1
result format for task results and run summaries. Local categories identify the kind of interaction;
they do not claim scores on the corresponding public datasets.

## Install once

Requirements: a Rust toolchain compatible with this repository, `uv`, Python
3.13 or later, and a Chromium-compatible browser. Linux and macOS are supported
by this harness's Unix-socket transport. Browser discovery checks
`OPENSIDE_PATCHED_BROWSER` (explicit opt-in), then `OPENSIDE_BROWSER`, then
Chromium/Chrome on PATH, and the standard Chrome application location on macOS.
Only disposable browser profiles are used.

Dependency installation may contact crates.io and PyPI. Do this before an
offline benchmark run, from the repository root:

```sh
cargo fetch --locked
uv sync --frozen --project agent
uv sync --frozen --project bench
```

If Python is not already installed, let `uv` install it during this preparation
step. The reproduction command disables Python downloads and dependency
network access. If using a custom `UV_CACHE_DIR`, use the same value for these
installation commands and subsequent runs.

## Reproduce with one command

```sh
bench/run.sh
```

The command builds `opensided` with `cargo build --offline --locked -j
${CARGO_BUILD_JOBS:-4} -p opensided --bin opensided`, discovers the sidecar
interpreter with `uv`, starts a temporary daemon and fixture server, runs all
five tasks, validates every
result with JSON Schema, prints the summary, and stops its daemon, sidecars,
browser descendants, and fixture server. No model credentials are read in the
default scripted mode. HOME, all XDG directories, workspace, socket, and browser
profiles are temporary and private. No broker or user's browser profile is used.

The scripted suite explicitly selects `orchestration = "single"` because its
fixed responses call tools directly. Live mode keeps the product's default
front-agent orchestration, with acting tools delegated to background Tasks.
Every task result and the run summary record the mode in `openside.orchestration`.

The benchmark run performs no external network requests: Cargo and uv are
forced offline; fixture pages use a restrictive same-origin CSP and contain no
external resources; daemon/sidecar communication uses a Unix socket; the fixture
HTTP server and browser CDP bind loopback. Browser policy allows only the fixture
origin, in both scripted and live mode. No remote websites or dataset APIs
are used. Missing offline dependencies or a missing browser fail with an
installation diagnostic, rather than yielding a misleading passing score.

Optional overrides:

```sh
OPENSIDE_BROWSER=/path/to/chromium bench/run.sh --output /tmp/bench-results
OPENSIDE_BENCH_DAEMON=/path/to/opensided bench/run.sh
```

`OPENSIDE_BENCH_DAEMON` skips only the build and must identify a real daemon
binary. `OPENSIDE_BENCH_PYTHON` may select an already installed sidecar
interpreter. Otherwise `uv run --offline --frozen --project agent` discovers it;
no interpreter or environment path is hardcoded. `CARGO_TARGET_DIR` is respected.
The selected output directory must be new or empty, so measurements from a
previous run cannot be overwritten accidentally. The default is a fresh
`bench/results/<UTC timestamp>/` directory.

## Results and schema provenance

Each task writes `<task_id>/result.json`, `events.jsonl`, `trajectory.txt`,
`stdout.txt`, and `stderr.txt`. `run_summary.json` records totals, categories,
pass rates, durations, model, provider, thinking level, concurrency, and UTC
timestamp. Artifact paths in each result are relative to its task directory.
`cliEvents` names the actual daemon SSE events even though this runner calls
HTTP directly, not the CLI.

`docs/specs/bench-schema.json` reconstructs the literal `TaskResult`,
`ModelSelection`, and `RunSummary` TypeScript interfaces from
[`at-inc/aside-benchmarks` at commit
`7cde728f6af4ba10065fc9e0557762d0e156d3dd`](https://github.com/at-inc/aside-benchmarks/blob/7cde728f6af4ba10065fc9e0557762d0e156d3dd/bu-bench-v1/runner/types.ts).
That repository publishes interfaces and example artifacts, not an upstream
JSON Schema. Its published artifacts can differ from the interfaces (for
example, some historical runs enable fast mode); this harness follows the
literal interface's `fastMode: false`. The schema is an openside reconstruction,
not an upstream-authored schema. Its `$comment` records the source and decision.

All openside-specific measurements are under the single `openside` extension:

- `suite`: `openside-local-web-v1`; `mode`: `scripted` or `live`.
- `grader`: `fixture-state-v1`, a deterministic local state/file grader.
- `orchestration`: `single` for scripted direct-tool responses; `front` for live
  mode using the product default.
- `steps`: number of actual daemon `tool.started` events, including the fake
  2FA ticket request. A REPL tool call can contain several browser actions.
- `tokens.input/output`: sums of actual daemon `usage` events. Scripted counts
  come from Pydantic AI FunctionModel's deterministic usage estimates, **not**
  paid/provider tokenizer counts; the `source` field makes that distinction.
- `browserBytes`: response-body bytes served by the loopback fixture for this
  task, including downloads; not browser RSS, HTTP header bytes, CDP bytes, or
  snapshot size. Every task has a separate URL prefix so the server attributes
  the traffic without a second measurement client.
- `ticketsResolved`: tickets actually auto-resolved by the harness. Only the
  local login task's one non-secret information ticket may be resolved; no
  unrelated confirmation or secret-input ticket is auto-approved.

`durationMs` measures the task submission-to-settlement wall time (plus the
bounded file-completion wait for download), excluding session creation and
initial daemon startup. `totalDurationMs` includes startup and cleanup.

## Tests

```sh
uv run --offline --frozen --project bench pytest bench/tests -m 'not live'
```

The end-to-end test invokes the one-command entrypoint from an unrelated working
directory, runs all five tasks, validates every JSON output, verifies real tool
and ticket events, reconciles token/step totals, and checks that benchmark
processes did not survive. Negative tests reject missing fields, invalid enums,
negative durations, malformed metrics, and malformed timestamps. Missing
Chromium or unsupported Windows transport prints a pytest skip reason; an
installed but failing browser/daemon/sidecar fails the test.

## Explicit live mode (not part of offline acceptance)

```sh
OPENSIDE_LIVE_MODEL='<provider>:<model-from-your-configuration>' bench/run.sh
```

This opt-in replaces the scripted FunctionModel with the requested real model
and may contact **that provider**. It still uses only local mock web tasks and
private HOME/XDG/browser directories. Only the standard provider API-key
environment variables are forwarded; broker, keyring, user CLI authentication,
and user configuration are not inherited. Live mode selects the existing
`ultrabrowse` profile, which explicitly requests high thinking; that setting and
the model/provider are recorded. Scripted mode has no thinking (`off`). Fast
mode is disabled in both modes. The live front agent starts Tasks for browser
actions rather than receiving direct access to acting tools; the benchmark does
not override this mode. The grader remains the same local deterministic
grader; no LLM grader is called. Live mode is not run by ordinary pytest and
should be explicitly budgeted by the operator.

Real Online-Mind2Web/BU Bench/Odysseys website evaluations are **the user's
separate action**: obtain the public datasets, website access and grader/model
credentials from their source projects, review their licenses and grading
configuration, and run their actual website evaluations explicitly. Do not
interpret this local suite's pass rate as a reproduction of any published
live results or a model-quality comparison.

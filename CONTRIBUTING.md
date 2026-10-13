# Contributing

openside is pre-alpha; expect large changes. Open an issue before starting on anything sizeable.

## Setup

You need the tools from the [README](README.md#build-and-run): Rust stable, a C compiler, uv, Node.js 22+ with pnpm 9, and a Chromium-based browser (browser tests print a skip reason without one). The full gate also needs [cargo-deny](https://github.com/EmbarkStudios/cargo-deny) on `PATH` or in `CARGO_DENY`; [actionlint](https://github.com/rhysd/actionlint) is used when present.

```bash
uv sync --project agent
uv sync --frozen --project bench
pnpm -C web install && pnpm -C extension install
```

## Checks

Run the whole gate before sending a change. It is what CI runs:

```bash
tools/gate.sh
```

It covers Rust fmt/clippy/tests, the sidecar (ruff, pyright, pytest), web and extension lint/typecheck/test/build, licence and `NOTICE` checks, the portability check, generated-docs drift, and the acceptance map in `docs/specs/acceptance.csv`. `OPENSIDE_GATE_JOBS` sets cargo parallelism (default 12).

Faster loops while working:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -j 2 -- -D warnings
cargo check --workspace --tests -j 2
cargo test -p <crate>
cd agent && uv run pytest -m "not live"
pnpm -C web test
```

If you change dependencies, regenerate `NOTICE` with `tools/check-licenses.py --write`. If you change the HTTP API or CLI, regenerate the reference with `tools/gen-docs.sh`.

## Licence

By contributing you agree that your contribution is dual licensed under MIT OR Apache-2.0, as described in the [README](README.md#license).

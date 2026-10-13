# Quality and operations checks

## Canonical gate and CI (PP-1)

`.github/workflows/ci.yml` runs `tools/gate.sh` on Linux x86_64 for pushes and pull requests. The gate includes Rust fmt/clippy/tests, sidecar ruff/pyright/non-live pytest, web/extension install/lint/typecheck/test/build, portability, Python tool tests, generated docs/API checks, language/license/browser-UX checks and acceptance evidence. Missing Chromium/live dependencies use the existing explicit test skip conditions; no live model credentials are configured in CI. CI uploads `target/gate` even after failure.

The gate invokes Vitest with default worker concurrency and both default and JSON
reporters. UI tests must wait for the state they assert, not just the element's
appearance: browser import profiles are loaded before an effect selects the
initial profile, so its radio query includes `checked: true`. A standalone
`--maxWorkers=2` run is not a substitute for the gate's command. Acceptance
references use the full nested suite path when they include suite names.

Configuration editors share an edit/save generation: an older response cannot
replace a newer draft, saved document, or refusal. Document refreshes preserve
editor identity instead of remounting it. Real-browser acceptance deliberately
switches to JSON before the form-save response arrives and delays both responses;
the local-authority refusal must remain visible without waiting for the earlier UI
save to finish. Account diagnostics
keep their five-second single-flight cache and 1 MiB output cap, but use the
larger of ten seconds and `agent.startup_timeout_ms` for the subprocess deadline.
The Python diagnostic entrypoint does not initialize the session runtime.

Thumbnails capture a full viewport JPEG without `clip.scale`, which temporarily
resizes Chromium's compositor surface and can poison concurrent preview frames.
Pure-Rust JPEG codecs perform scaled DCT decoding into a 360x225 thumbnail buffer
and re-encode it off the async executor; the decoder's Rayon pool is disabled.
Browser regression coverage observes window and visual-viewport resize events,
samples layout/viewport/scale/media values during capture, and requires JPEG
dimensions to match every preview frame declaration under thumbnail overlap.
Preview captures bind to the cached top-frame loader identity. Committed
navigation retires the previous capture and its queued frames rather than
inheriting exhausted ACK credits; the normal tick starts the new capture without
DOM queries. Paced casts request `sendLastFrame` on newer Chromium to retain the
latest static repaint while waiting for credit.
The frame deadline belongs to the feed, not to a particular document capture:
restarting a capture must not reset it. Otherwise pages navigating faster than
the 500 ms cadence postpone every frame indefinitely. A regression keeps the
page navigating every 350 ms and requires three changing JPEG frames.
Remote-object cleanup is idempotent only for Chromium's specific destroyed
context/object errors after a successful operation reply. An auto-submitting
code field must not turn an acknowledged fill into a false refusal; operation
failures and unrelated cleanup/transport failures still propagate.

Takeover completion replaces the pending request with a receipt and dismisses its
remote-control dialog. UI and relay acceptance require both the resolved
"You finished the step" receipt and the absence of the dialog after Done;
they check the Close control while the takeover is still active, not afterward.
Artifact previews open in a modal files sheet, which intentionally covers the
conversation. Relay acceptance checks artifact pixels inside the sheet, then
dismisses it before checking conversation visual pixels; both sandbox loads,
inline styles, opaque origins, and script/network refusal remain covered.
Pixel sampling waits for ancestor CSS transitions to finish; two animation
frames alone do not guarantee the dialog reached full opacity.
Vault policy acceptance opens the labelled shared combobox and chooses an
option, then checks the stored policy, effective policy, selected label, and
audit record. Extension glass acceptance targets the custom session-picker
button rather than a removed native select.
Configuration refusal coverage records the actual `local_access_required` code
and requires its visible guidance while retaining the delayed-response ordering
and durable no-mutation checks.
The app root uses Base UI's `CSPProvider disableStyleElements`; library-injected
scrollbar `<style>` tags otherwise violate the daemon's `style-src 'self'`.
The stylesheet-only configuration keeps that policy unchanged, and admin
acceptance requires an empty violation list after opening the shared Select.

Two acceptance rows require checks outside the ordinary Cargo suite. The
`responsive first browser action` step runs only
`responsive::cold_and_warm_first_browser_action` with `--ignored --exact`,
retaining its actual Cargo output in `responsive.log` and JUnit in
`responsive.xml`. It fails if the test is missing, ignored, or reports a browser
skip. Cargo jobs follow `OPENSIDE_GATE_JOBS`, then `CARGO_BUILD_JOBS`, then four;
the focused test uses two test threads.

The `browser UX evidence` step first checks the UX evidence registry, then runs
`tools/qa/headed-ux.sh sway stock --prewarm-only` against the E2E daemon binary.
This rootless Podman run has no network and uses its own headless sway; it never
uses the host's desktop or daemon. All four prewarm/focus checks must complete
and pass before `headed-pytest.xml` records `tools.qa.headed_ux::prewarm_checks`
as passed. Screenshots, compositor focus changes and JSON results are retained
under `target/gate/sway/stock`. CI installs Podman with bubblewrap; the stock-only
run does not require a patched browser build. Local Chromium/Cargo invocations
must still use the workstation's `/tmp/openside-heavy-run` wrapper.

The same workflow has native `macos-latest` and `windows-latest` matrix runners invoking `cargo check --workspace --all-targets --locked -j 8`. Local workflow validation uses `tools/check-ci.py`: uv-managed PyYAML loading and dependency-free structure/canonical-contract validation always run, and actionlint adds syntax validation when installed. Either validator's failure fails the check; absent actionlint is explicitly reported and syntax certification is not claimed. The contract function is directly importable by pytest without PyYAML or jsonschema. Workflow syntax is not proof of a successful hosted run; this checkout has not been pushed or committed by the quality slot.

## Languages and platform boundaries (G-1, G-3)

`tools/check-languages.py` enumerates Rust code in every crate, Python only in the credential/runtime sidecar, and TypeScript only in web/extension. Installed/generated outputs are excluded, not source files. `openside-repl/src/prelude.js` is an explicitly documented sandbox guest API embedded by the Rust controller, not a separate JS controller. Only named ESLint configuration files and the web service worker are JavaScript UI-asset exceptions; arbitrary root-level JS implementations are rejected. A foreign implementation file causes failure.

Platform isolation exists in Vault's `PlatformLock` trait (including its wrapping-key interface) and cfg-selected Secret Service/native keyring implementations (`crates/openside-vault/src/platform.rs`, `platform/native.rs`), config discovery/paths/doctor cfg branches, the sidecar's Unix process module/non-Unix error implementation, daemon Unix transport/shell cfg branches, and the browser gateway's Linux/non-Linux cfg implementations. These boundaries do not claim that every crate already compiles or runs on Windows.

`tools/check-platforms.py` checks **every workspace crate and all Cargo targets**, not a selected crate list. It first uses installed target std; when that is missing it discovers an already-installed nightly with `rust-src` and invokes `cargo +<nightly> check -Zbuild-std=std,panic_abort --workspace --all-targets --locked -j 4 --target-dir <temporary-directory>/openside-xcheck`. Targets cover x86_64/aarch64 macOS and both MSVC/GNU Windows. It never installs toolchains, target std, compilers or SDKs. Missing std/rust-src or recognized missing cross-compiler/SDK diagnostics are printed as `BLOCKED-EXTERNAL` with the exact build output and a `User action:`; Rust diagnostics and other compiler failures fail the gate. `CC_<target>`/`AR_<target>`, SDK flags and cross-toolchain `PATH` entries are inherited from the caller. A build-std compile check exercises the target's real Rust std surface and native C build scripts, but is not a link/run test or proof that the pinned stable toolchain passes on a native runner. The unchanged native CI matrix remains that final check. The headless Linux browser test `crates/openside-browser/tests/platforms.rs` runs real Chromium under both Wayland and X11 session metadata with deliberately invalid display endpoints. This proves core operation does not require a desktop connection, not that native window-manager focus or headful compositor behavior has been certified.

## Dependencies and NOTICE (PP-2)

Install cargo-deny with `CARGO_INSTALL_ROOT=/tmp/cargo-bin cargo install cargo-deny --locked -j 8`, or install it normally in a development/CI environment. Run `CARGO_DENY=/tmp/cargo-bin/bin/cargo-deny tools/check-licenses.py` for the private installation. The check runs cargo-deny advisories/licenses/bans/sources using `deny.toml`, validates installed web/extension npm license expressions through `pnpm licenses list --json`, reads every `agent/uv.lock` and `bench/uv.lock` package from the installed `.venv` metadata (version must match the lock), and compares generated NOTICE content byte-for-byte. Install both JS lockfiles and run `uv sync --frozen` for `agent` and `bench` first. Regenerate with `tools/check-licenses.py --write` after a dependency update.

The permissive allowlist does not allow GPL/LGPL-only dependencies. Dual-license r-efi is selected under MIT. Unmodified option-ext (Rust), certifi and hypothesis (Python) are the only MPL exceptions; their file-level license/source obligation is retained in NOTICE. npm build-time Python-2.0/CC-BY data and permissive BlueOak packages are explicitly allowed; Python adds PSF-2.0 and CNRI-Python. Python packages whose metadata has only free text or the bare word "BSD" are mapped in `tools/check-licenses.py` after reading the licence text they ship; packages locked only for other platforms (win32, emscripten) carry a reviewed expression. Unknown/empty SPDX expressions fail closed; AND requires both licenses, OR permits an allowed choice. NOTICE includes upstream package versions, license expressions, copyright/license/NOTICE file contents when supplied, vendored material (aside-relay, aside-remote, fonts, the BU Bench V1 result schema reconstruction) and design-only references (xplorer); browser overlay notices remain `browser/NOTICE.md`.

The IJG allowance is scoped to jpeg-encoder only. Generated NOTICE retains the
original DCT source header's copyright and no-warranty text, plus the required
executable-distribution acknowledgment; it is not a license-wide exception.

## Local crash reports

The daemon binary installs a panic hook after opening its private data directory. It replaces the default stderr hook, because panic messages/backtraces can contain credentials. `data_dir/crash-reports/panic-<time>-<pid>.json` contains only format, fixed panic kind, timestamp, PID, source basename/line, redaction markers and `uploaded:false`. Reports are 0600 under a 0700 directory on Unix. Panic payload, thread name, environment and backtrace are not persisted or printed. `openside doctor --json` lists local report paths without opening their contents and never uploads them.

SIGKILL cannot execute a panic hook. Native Chromium minidumps and operating-system core dumps can contain secrets and are **not** collected by this mechanism; there is no claim of native dump symbolization or remote crash telemetry. Report retention is local/manual; inspect and delete metadata reports when no longer needed. The storage table is generated from the typed registry in `docs/specs/data-layout.md`; a real daemon/browser lifecycle test audits every created data/config/runtime/cache path against it.

## Soak (PP-23)

`tools/soak.sh` runs the default-ignored Rust `soak` E2E test. Default: 900 seconds and at least 10 sessions. Each loop starts a real daemon session and scripted Python sidecar, drives loopback Chromium, SIGKILLs the sidecar and browser, verifies replacement/recovery, then stops/deletes the session. Every fifth loop runs and removes a routine; every third loop abruptly restarts the daemon against its durable storage. No live LLM/network service is used.

The harness fails on a missing browser/sidecar, unexpected runtime/tool error, unplanned browser-process replacement, missing completed reply, recovery deadline, daemon panic/report, cleanup failure or resource-bound violation. Browser PID continuity is asserted between declared fault/restart boundaries, so automatic recovery cannot silently hide a spontaneous browser restart. RSS and fd samples are taken from the daemon after lifecycle cleanup, with a baseline per daemon generation; maximum allowed growth is 128 MiB and 32 descriptors. Those bounds do not claim a browser-descendant aggregate memory limit. `target/soak-summary.json` records duration, session/fault/restart/routine counts, unexpected crashes and resource samples.

On Linux the harness makes the test process a child subreaper, so every orphan of the daemon's process tree is re-parented to it: renderers outliving a SIGKILLed browser group, Chromium's double-forked `chrome_crashpad_handler`s (their own session, outside the browser's process group), a browser adopted across a daemon restart. `openside-e2e` reaps those on every SIGCHLD (plus a 1 s pass). It never calls `waitpid(-1)`, which would steal exit statuses from the test process's own tokio children; it reaps only zombies that cannot be such a child (members of a foreign process group, and owned group leaders it did not spawn itself). Without this, a 4-hour soak accumulated about 16,000 zombies, and a restarted daemon closing an adopted browser timed out, because a zombie still answers `kill(pid, 0)`. `crates/openside-e2e/tests/browser_zombies.rs::browser_kill_relaunch_and_close_cycles_leave_no_zombies` covers kill, relaunch, adopt and close cycles. The installed daemon is not a subreaper: its orphans go to the service manager (`systemd --user`), which reaps them, and tokio reaps the daemon's own `Child`ren.

Literal 24-hour certification remains blocked-external. User action: `OPENSIDE_SOAK_SECONDS=86400 OPENSIDE_SOAK_SESSIONS=100 tools/soak.sh` on a machine with the sidecar and Chromium installed, then retain the generated summary. The compressed run is not represented as a 24-hour run.

### Compressed run evidence

Executed `tools/soak.sh`: 901.675407979 seconds of sampling, 159 sessions, 159 injected sidecar SIGKILLs, 159 injected browser SIGKILLs, 53 daemon restarts, 32 routine runs, **0 unexpected crashes**. Maximum within-generation daemon RSS growth: 1,696 KiB (limit 131,072 KiB); fd growth: 1 (limit 32). The test passed and its owned processes were reaped. Browser PID continuity held outside injected-fault boundaries. The recorded JSON is `docs/specs/soak-result.json`; all 159 per-session samples are retained in that artifact. This is compressed Linux evidence, not 24-hour or macOS/Windows certification.


# Bot-detection score records (B-6 / BR-2)

## Scope and provenance

The local suite is an independently written set of detection-signal fixtures, not a copy or a mirror of a public site's test code. It serves three pages on an ephemeral `127.0.0.1` port and uses disposable profiles. The default runner never navigates to a public page. The P7 browser patch scope is to remove automation artifacts, not to fabricate fingerprints.

Public pages are **provenance only** in the local suite:

- **Intoli Chrome Headless Test**: <https://intoli.com/blog/not-possible-to-block-chrome-headless/chrome-headless-test.html>. Its [source README](https://github.com/intoli/intoli-article-materials/tree/master/articles/not-possible-to-block-chrome-headless) describes the classic headless-test family.
- **Sannysoft Bot Test**: <https://bot.sannysoft.com/>. A broader presentation of classic navigator, permission, WebGL, frame and native-integrity signals.
- **Device & Browser Info — Are You a Bot?**: <https://deviceandbrowserinfo.com/are_you_a_bot>. Provenance for headless user-agent/client-hint and inspector-console signals.
- **Rebrowser Bot Detector**: <https://bot-detector.rebrowser.net/>. Its [source README](https://github.com/rebrowser/rebrowser-bot-detector) explicitly describes `runtimeEnableLeak`, `navigatorWebdriver`, viewport and automation-global checks.

Only the conceptual signals are reproduced. No third-party detection implementation is copied, and none of these public pages is exercised by the automated local tests. Site-specific algorithms, classifications and thresholds are not claimed to be identical. The new Rust and inline fixture code follows the repository's MIT OR Apache-2.0 license; the provenance links impose no copied-code license dependency.

## Local checks

Each page publishes an array of `{id, passed, observed}` objects. Exceptions become a failing check with the error in `observed`; missing, duplicate or unexpected IDs fail the run rather than reducing its denominator.

| Page | Check ID | Pass condition | Signal provenance | P7 gate |
| --- | --- | --- | --- | --- |
| classic | `navigator.webdriver` | Native getter returns `false` | Intoli, Sannysoft, Rebrowser | yes |
| classic | `user_agent.headless` | UA does not contain `HeadlessChrome` | Intoli, Sannysoft, Device & Browser Info | yes |
| classic | `client_hints.headless` | Nonempty UA brands do not advertise headless | Device & Browser Info | yes |
| classic | `window.chrome` | `window.chrome` is an object | Intoli, Sannysoft | no |
| classic | `navigator.plugins` | Native PluginArray with at least one plugin | Intoli, Sannysoft | no |
| classic | `navigator.mime_types` | Native MimeTypeArray with nonempty, plugin-linked entries | Sannysoft | no |
| classic | `navigator.languages` | Nonempty languages agree with `navigator.language` | Intoli, Sannysoft | no |
| classic | `permissions.notifications` | Notification.permission agrees with permissions.query | Intoli, Sannysoft | no |
| classic | `webgl.identity_present` | A real WebGL context returns nonempty vendor and renderer | Sannysoft | no |
| classic | `window.dimensions` | Positive inner/outer dimensions; outer covers inner | Sannysoft, Rebrowser viewport family | no |
| integrity | `window.automation_globals` | No known `cdc_`, WebDriver, Selenium, Playwright or Phantom globals | Sannysoft, Rebrowser | no |
| integrity | `iframe.content_window` | A same-origin frame has a distinct, self-consistent window and matching webdriver state | Sannysoft | no |
| integrity | `native.to_string_integrity` | Selected navigator getters, permissions.query and Function.toString have native source consistent with a fresh iframe realm | Sannysoft | no |
| runtime | `cdp.runtime_console_getter` | Console Error.stack getter is never read **and** the controller never sends Runtime.enable | Device & Browser Info, Rebrowser | yes |
| native gateway | `browser.automation_infobar` | Authenticated browser.status reports automation_infobar_count == 0 | P7 native UI requirement, not a public-page signal | yes |

The console probe emits the same Error twenty times on a 25 ms timer to cover attachment timing. Its getter is ordinary page JavaScript, not injected controller code. Chromium versions may no longer preview Error.stack; therefore a false getter signal alone is not evidence that Runtime.enable is absent. The runner also records the controller's actual outgoing method log, and the P7 check fails if Runtime.enable was sent. The log itself is not persisted, only its Boolean observation.

Native infobars cannot be read by web-page JavaScript. Gateway mode obtains their real count through `Gateway::request("browser.status", {})`; unsupported or malformed observations fail this check. Stock CDP has no corresponding native observation: its score has 14 checks and `threshold.not_observed` explains the omitted infobar check. This is not a fabricated pass. Gateway scores have 15 checks. A zero native count proves absence in the tested headless launch, not a headed desktop inspection or every possible launch switch.

All three fixture tabs remain open until the native status is sampled, so closing their windows cannot erase an infobar before it is counted.

These are narrow signal checks, not proof of being human or invisible to anti-bot services. Empty plugins, denied notifications, unavailable GPU contexts and unusual dimensions can legitimately occur with stock browsers or restricted machines. They remain diagnostic results, not P7 failures. GPU/vendor impersonation, canvas/audio noise, proxy reputation and behavioral camouflage are out of scope. Native-source integrity detects ordinary JavaScript shims but is not a complete proof against all Proxy or realm tampering.

## Runner and threshold

```bash
# Both modes when available; no public navigation.
./bench/botcheck/run.sh --output /tmp/botcheck-local.json

# Require the patched mode explicitly.
export OPENSIDE_PATCHED_BROWSER="$OPENSIDE_BROWSER_BUILD_DIR/portablelinux/build/src/out/Default/chrome"
./bench/botcheck/run.sh --mode patched --output /tmp/botcheck-patched.json

# Deliberately leaky stock launch, for detector calibration.
./bench/botcheck/run.sh --mode negative --output /tmp/botcheck-negative.json

cargo test --offline --locked -j 8 -p openside-browser --test botcheck -- --nocapture
```

The shell wrapper runs the Rust `openside-browser` example offline. Dependencies must already be installed. Stock discovery checks PATH for `chromium`, `chromium-browser`, `google-chrome`, `google-chrome-stable`, then `chrome`. Patched discovery uses only `OPENSIDE_PATCHED_BROWSER`. An unset patched variable, missing patched file or absent stock executable emits a specific skip reason. A discovered binary that cannot launch is an error, not a skip.

Both modes reuse `Browser::launch`, `create_agent_tab`, `snapshot`, `evaluate` and `close_agent_tab`, the same controller path used by the daemon. No UA override, stealth flag, injected getter shim, custom GPU configuration or suppression of normal signals is added. Gateway mode uses the daemon's `opensided` scope and a fresh private runtime directory; it is required, never silently replaced with stock CDP. The only lifecycle differences are disposable profiles and non-detached cleanup instead of the daemon's durable profile and restart-surviving process. Negative mode uses the same launch through a temporary shell wrapper adding only `--enable-automation` to stock headless Chromium.

**Decision B6-THRESHOLD:** patched gateway mode must pass **all five P7 checks** in the table. Stock and negative scores are recorded but not gated. The broader score is `passed / total`; there is no arbitrary overall percentage cutoff, because diagnostic fingerprint variation is outside P7. A failed required native observation also fails the patched gate.

Exit codes: `0` means at least one browser ran, every gated P7 result passed and every requested public extraction succeeded; `1` means a patched gate failed or a requested public extraction failed; `2` means invalid arguments, missing prerequisites, a runner error, or no available browser. Score JSON is written before returning a threshold/extractor failure. Operational errors may prevent producing a complete score record.

## JSON schema (version 1)

The output is a UTF-8 JSON object:

- `schema_version`: integer `1`.
- `suite`: `"openside-local-botcheck"`.
- `recorded_at_unix_ms`: integer Unix timestamp in milliseconds.
- `skipped`: array of explicit browser-absence reasons.
- `records`: one object per executed mode:
  - `browser_version`: actual executable `--version` output.
  - `mode`: `stock_cdp`, `patched_gateway`, or `negative_stock_automation`.
  - `checks`: array of `{id: string, passed: boolean, observed: object}`. Observations contain the actual signal values or an error; no browser profile or authentication token is recorded.
  - `score`: `{passed: integer, total: integer, ratio: number}`; ratio is `passed / total`, between 0 and 1.
  - `runtime_enable_sent`: Boolean from the real controller method log for the local pages.
  - `threshold`: `{gated: boolean, passed: boolean, required_check_ids: string[], not_observed: string[]}`. `passed` means all five required checks were observed passing, even when `gated` is false. Stock's absent native observation therefore leaves it false, without failing the process.
  - `public_pages`: an array described below, empty unless explicitly opted in.

Example fragment (illustrative, not a claimed run):

```json
{"id":"navigator.webdriver","passed":true,"observed":{"value":false}}
```

## Explicit public-page opt-in

Public evidence for B-6 remains separate from local BR-2 evidence: public results are never mixed into the local denominator or the P7 threshold, and no public-page result is fabricated from a local fixture. The default runner never visits a public site; `--public-urls` is an explicit opt-in. A real public-site run (patched vs stock) was made on 2026-10-12 with the user's authorization (read-only visits, no logins, no forms); its results are in "Public-page results (2026-10-12)" below.

```bash
# Review the selectors in this example manifest first; they were checked against the live pages on 2026-10-12 and live pages change.
./bench/botcheck/run.sh \
  --public-urls bench/botcheck/public-pages.example.json \
  --output /tmp/botcheck-public.json
```

`--public-urls FILE` is the only external-navigation opt-in. FILE contains a nonempty JSON array of per-page entries with unique `id`, HTTPS `url`, `extractor`, and optional `timeout_seconds` (default 30, range 1..300). The bundled example names the Intoli classic result table and two Sannysoft tables (the classic rows and the fpscanner `#fp2` table). Its selectors were verified against the live pages on 2026-10-12 and the manifest runs green through `run.sh`, but live DOMs change, so review them before relying on a new run. The extractor specification is recorded with each result so selector changes cannot silently masquerade as the same score. Public pages can load their own third-party resources; unlike the local fixtures this is not a hermetic-network run. Temporary profiles are still used, and no saved user session is accessed.

The Intoli entry (`table tr:has(td)`, result cell `td.result`) classifies by the `passed`/`failed` classes: the live page's script sets one of the two on every row, so it is an explicit pass/fail table (the failure-only convention of the GitHub source does not match the live page). The `sannysoft-classic` entry (`table:not(#fp2) tr:has(td.result)`) replaces the former `#main-table`, which does not exist on the live page (it has two unnamed tables plus `#fp2`); `sannysoft-fpscanner` reads the 20 fpscanner rows from `#fp2` (`td:nth-child(2)`). A fpscanner row can also carry a `warn` class; the extractor reports an unmatched class as `extraction_error` rather than counting it as pass or fail. None occurred in the observed runs.

Supported per-page extractors:

1. **`table`**: `row_selector` selects only test rows; `result_selector` selects the result cell within each row; disjoint `pass_classes` and nonempty `fail_classes` classify that cell's actual CSS classes. A row must match exactly one outcome and have nonempty result text. For a documented failure-only table, `pass_when_no_fail_class: true` explicitly means a populated cell without a failure class is a pass; only then may `pass_classes` be empty. This is an opt-in convention for tables that mark only failures, not a generic assumption about all pages; the bundled Intoli entry no longer uses it because the live page marks both outcomes. The score is the number of passing rows divided by the number of rows, preserving labels, raw result text and classes. This is suitable for the classic Intoli/Sannysoft pass/fail tables; it is not described as an official aggregate score if the page does not expose one. Unclassified, conflicting, empty or missing rows are never counted as passes.
2. **`numeric`**: `passed_selector` and `total_selector` identify elements containing numeric text only. Both must be finite, total > 0 and 0 <= passed <= total. The runner preserves the raw text and records the page's exposed passed/total score. Example entry for a user-inspected page:

   ```json
   {"id":"my-score-page","url":"https://example.org/score","extractor":{"kind":"numeric","passed_selector":"#passed","total_selector":"#total"}}
   ```

Extraction polls until the document is complete and the selected values are unambiguous, bounded by the page deadline. A CAPTCHA, changed markup, unsupported score format or missing selector returns `status: "extraction_error"`, `score: null`, and the error, not an invented zero or pass. No login, challenge bypass, page instruction following or arbitrary user-supplied JavaScript extractor is provided. The user should update selectors or add a declarative supported extractor after inspecting the page rather than treating an extraction error as anti-bot success.

Each `public_pages` result has `id`, `url`, `extractor`, `status` (`scored` or `extraction_error`), `observed` (raw table rows, raw numeric text, or error), and `score` (`{passed,total,ratio}` or null). Public results are not mixed into the local denominator or the P7 threshold.

## Public-page results (2026-10-12)

One manual run with the user's authorization (read-only visits, no logins, no forms), disposable profiles, stock `Chromium 152.0.7977.82` vs the patched `Chromium 155.0.8059.39` (gateway mode), through `Browser::launch` (the daemon's launch path). Headless is the production launch; headed used a wrapper that only drops `--headless=new`. Counts are rows or signals of each page's own table.

| Site | stock headless | patched headless | stock headed | patched headed |
| --- | --- | --- | --- | --- |
| bot.sannysoft.com, Intoli rows (`td.result`) | 6/8 (user agent HeadlessChrome, WebDriver present) | **8/8** | 7/8 (WebDriver) | **8/8** |
| bot.sannysoft.com, fpscanner (`#fp2`) | 18/20 (HEADCHR_UA, CHR_MEMORY) | **20/20** | 20/20 | **20/20** |
| Intoli chrome-headless-test | 4/6 | **6/6** | 5/6 | **6/6** |
| CreepJS, Headless block | 100% headless | **0% headless** | 33% headless | **0% headless** |
| Pixelscan bot-check | Bot Behavior Detected (Webdriver, CDP) | **Human** | Bot Behavior [INFERENCE: from page text] | Human [INFERENCE: from page text] |
| BrowserScan bot-detection | Robot | **Normal** | Robot | Normal |
| deviceandbrowserinfo are_you_a_bot | isBot true (strong signals) | isBot true, only `hasSuspiciousWeakSignals` | isBot true | **isBot false** |
| bot-detector.rebrowser.net | webdriver and useragent red | runtimeEnableLeak and webdriver pass, useragent red | webdriver and useragent red | runtimeEnableLeak and webdriver pass, useragent red |
| arh.antoinevastel.com areyouheadless | HTTP 502 | HTTP 502 | HTTP 502 | HTTP 502 |

The local suite reproduces the documented baseline in both modes: patched 15/15 (headless and headed), stock 10/14 headless and 11/14 headed. The official command `./bench/botcheck/run.sh --public-urls bench/botcheck/public-pages.example.json` exits 0 (local stock 10/14 and patched 15/15; public Intoli 4/6 vs 6/6, Sannysoft 6/8 vs 8/8, fpscanner 18/20 vs 20/20).

Weak signals that stay red on the patched build and are not P7 automation artifacts:

- **SwiftShader and an 800x600 screen in headless.** `deviceandbrowserinfo` calls the headless patched run a bot only through `hasSuspiciousWeakSignals`: the WebGL renderer is `ANGLE (... SwiftShader ...)` and `screen` is 800x600 with `availWidth`/`availHeight` equal to the screen. On a real GPU in headed mode the flag is false and `isBot` is false. The same cause turns the Sannysoft "WebGL Renderer" row red (10/11 headless patched, 11/11 headed) and CreepJS `hasSwiftShader`. GPU/vendor impersonation is out of scope.
- **Unbranded user agent brands.** `navigator.userAgentData.brands` has `Chromium` and `Not(A:Brand` but no `Google Chrome` on both builds, so the rebrowser `useragent` row is red. Fixing it would be brand spoofing, which is out of scope.
- **CreepJS "like headless" 38-50%** on the patched build comes from Chromium-on-Linux-desktop properties shared with stock (known background color, no taskbar, no Web Share, no Contacts Manager); the decisive `% headless` figure is 0% for patched. This reading uses an approximate reimplementation of CreepJS's feature checks [INFERENCE: not exact parity].

Method limits: a headed stock window on a hidden compositor workspace does not present frames (the page stays on "Loading..."), so some stock headed text came from a second run; gateway `screenshot.capture` in a headed dedicated agent window used to fail with `surface_unavailable: viewport is not 1440x900` (fixed afterwards, see PROGRESS G3-D44). The raw evidence (per-site `report.json`, page text, screenshots, `runsh-public.json`) is under `/tmp/openside-qa/botcheck/`; it is not copied into the repository because there is no results directory convention for manual network runs.

## Executable evidence and acceptance

`crates/openside-browser/tests/botcheck.rs` contains real-browser tests:

- `local_stock_records_real_detection_signals`: executes all fourteen local page checks through stock CDP and records the normal Runtime.enable path.
- `local_patched_passes_every_p7_automation_artifact_check`: executes all fifteen checks in required gateway mode and asserts all five P7 checks, including the native count.
- `deliberately_leaky_stock_configuration_is_detected`: launches stock with --enable-automation and asserts webdriver, headless UA and the Runtime-domain observation fail. This proves the detector does not unconditionally return success.
- `public_extractors_read_real_loopback_page_scores`: extracts 1/2 from both explicit-class and failure-only real loopback tables and 7/10 from numeric HTML. It proves missing selectors and unfinished failure-only tables produce extraction errors without scores. It exercises public extractor code without visiting public sites.

Tests print explicit skip reasons only when the needed browser binary is unavailable. Local successful runs satisfy the automation-artifact portion of BR-2. B-6 is **pass**: the local tests above stay the executable evidence in the gate, and the manual public-page run of 2026-10-12 (it needs the network, so the gate does not repeat it) shows the patched build clean on the automation signals where stock fails. To repeat it, run the public command above and keep the JSON result; do not relabel local evidence as public-page verification.

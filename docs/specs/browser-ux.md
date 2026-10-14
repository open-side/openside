# Browser UX acceptance decomposition

B-8 defines the decomposition below, not a blanket claim that browser-chrome features are implemented. Source: `docs/01-requirements.md` B-8, the patched-browser items P8, P9 and P11, and the stock-browser extension. `implemented` identifies an existing test surface; native Chromium UI items without implementation are stated explicitly rather than represented by mock tests. Evidence existence is enforced by `tools/check-browser-ux.py` in the gate; behavioral results come from the corresponding test runner.

| ID | Item | Acceptance criterion | Surface | Status | Evidence |
|---|---|---|---|---|---|
| UX-01 | Vertical tabs | A user can switch between horizontal and vertical tabs; selection, order and ownership are retained | Native Chromium P9 | needs-test | `crates/openside-browser/tests/patched.rs` (test native_ux_defaults_are_enabled: the upstream vertical-tabs preference defaults on and is observed through browser.status; headed toggle/order/ownership interaction not yet exercised) |
| UX-02 | Split view | A user can show two selected tabs side by side, resize and exit without closing either tab | Native Chromium P9 | needs-test | `crates/openside-browser/tests/patched.rs` (test native_ux_defaults_are_enabled: upstream split view feature observed enabled; headed split/resize/exit interaction not yet exercised) |
| UX-03 | Bookmark bar | A user can show/hide the bar and open a saved bookmark without moving agent-tab ownership | Native Chromium | not-implemented | — |
| UX-04 | Bookmark sidebar | A user can open the sidebar, search bookmarks and open one in a user-owned tab | Native Chromium P9 | needs-test | `crates/openside-browser/tests/patched.rs` (test native_ux_defaults_are_enabled: upstream bookmarks side panel observed registered; headed open/search/open-tab interaction not yet exercised) |
| UX-05 | Pinned tabs | Pin/unpin persists URL and display title and participates in encrypted two-device synchronization | Daemon API | implemented | `crates/openside-e2e/tests/sync.rs` |
| UX-06 | Rename tabs | A renamed tab retains its custom name after reload/restart without rewriting page content | Native Chromium | not-implemented | — |
| UX-07 | AI Tidy | A user explicitly requests tab organization; proposed groups are shown for approval and no user tab is closed automatically | Native Chromium | not-implemented | — |
| UX-08 | Profile switching | A user can select a managed profile and sees its separate history, cookies and bookmarks; an existing personal profile is never silently adopted | Native Chromium / import | not-implemented | — |
| UX-09 | PiP preview | A browser JPEG preview can be minimized and expanded; the minimized preview accepts no page input | Web UI | implemented | `web/src/components/TakeoverView.test.tsx`; `crates/openside-e2e/tests/takeover.rs` |
| UX-10 | Ask side panel | The panel creates a session for the current tab, submits a question and renders the streamed answer; tab changes do not reuse stale context | MV3 stock-browser fallback | implemented | `extension/tests/panel.test.ts`; `crates/openside-e2e/tests/extension.rs` |
| UX-11 | Address-bar Ask AI | Entering the extension keyword and confirming the question opens Ask with the original question; the native address-bar Tab transition is not claimed by this fallback | MV3 omnibox keyword / patched P8 ask keyword | implemented | `extension/tests/context.test.ts`; `extension/tests/build.test.ts`; `crates/openside-browser/tests/patched.rs` (test daemon_ui_new_tab_and_routines_open_existing_web_ui: the opt-in ask keyword opens the existing web UI; the question is not auto-submitted) |
| UX-12 | Explicit @ context | Only an explicit context request attaches current title, URL and selection; page text is marked untrusted and token material never enters it | MV3 Ask | implemented | `extension/tests/context.test.ts`; `extension/tests/panel.test.ts` |
| UX-13 | Mini popup | A user can open a compact Ask popup, submit a question and dismiss it without activating or closing the underlying agent tab | Native Chromium P8 | not-implemented | — |
| UX-14 | New-tab Ask page | A new user tab offers Ask and starts a daemon session through the same public API; native page assets obey CSP | Native Chromium P8 (opt-in daemon UI switch) | needs-test | `crates/openside-browser/tests/patched.rs` (tests daemon_ui_new_tab_and_routines_open_existing_web_ui and daemon_ui_rejects_credentials_query_and_nonroot_switch_values: a non-incognito new tab opens the existing web UI from a credential-free loopback URL; starting a session from that page is the web UI's own flow and is not exercised in the patched test) |
| UX-15 | Agent Tabs | Only agent-owned targets join Agent Tabs; external MCP tabs join the same group and existing user groups remain untouched | MV3 / patched Chromium | implemented | `extension/tests/agent-tabs.test.ts`; `crates/openside-e2e/tests/mcp_tabs.rs` |
| UX-16 | Scheduler UI | A user lists routines, creates/edits a schedule and runs it now through the daemon API; previous runs remain visible | Web UI / native P11 | needs-test | `web/src/pages/RoutinesPage.test.tsx`; `web/src/App.manage.test.tsx`; `crates/openside-e2e/tests/routines.rs`; `crates/openside-browser/tests/patched.rs` (web create/list/run/history covered; the patched routines keyword opens the existing /routines page and lists daemon API routines; schedule editing not certified) |
| UX-17 | Handle with openside | A trusted user gesture on the Gmail/Slack mock-page action sends only explicitly selected, untrusted page context to Ask; synthetic page clicks cannot submit or access credentials | MV3 stock-browser fallback / A-14 | implemented | `extension/tests/content.test.ts`; `crates/openside-e2e/tests/extension.rs` |

Limitations: PiP here means the app's read-only minimized preview, not operating-system video PiP. Pinned-tab API coverage is not native tab-strip pin/unpin coverage. Scheduler web create/list/run/history coverage does not prove schedule editing. The patched P8/P11 entry points open the existing web UI (no privileged WebUI, no native Ask side panel); the native Ask side panel (UX-10 native parity) is not implemented and the MV3 side panel remains the Ask surface. P9 native UX items are observed enabled headlessly; headed interaction tests are still required before `implemented`. User action: run the headed vertical-tabs/split-view/bookmarks-sidebar interactions in the patched build before promoting UX-01, UX-02 and UX-04. Native items marked `not-implemented` need implementation plus a real UI test before their status can change.

## Safe page variants and continuation

Browser lifecycle URLs continue to omit credentials, every query parameter and
fragments. Browser state and browsing-history identity use a separate display
facet allowlist; they never copy arbitrary query strings. The first rule is:

| Host and scheme | Paths | Key | Allowed values | Missing key |
|---|---|---|---|---|
| `https://github.com` (default port) | `/trending`, `/trending/developers` | `since` | `daily`, `weekly`, `monthly` | `daily` |

Unknown keys, tokens and fragments are discarded. An unrecognized value or
duplicate `since` key receives no facet. Other hosts, ports and paths receive
no facet. Browser/status lines name the period, and daily and weekly visits have
different history identities; omitted `since` and explicit `since=daily` share
the daily identity. Existing query-stripped history is not reconstructed.
The Rust allowlist and web status formatter must change together when adding a
known page pattern. Regression evidence is in `browser.rs`'s `display_facets_`
tests and `web/src/components/toolStatus.test.ts`.

User-facing messages, `status_text` and narration follow the person's latest
explicit language preference, otherwise their request's language. A page or
tool's language does not override it; identifiers and quotations stay verbatim.
Only `assistant.final` text is rendered in the transcript, including non-front
sessions. Provisional deltas remain internal and are not translated into replies.

Browser notices for Task-owned tabs steer that Task, never the front agent.
Terminal Tasks receive no notices, including late download completion or close
events. Daemon cleanup closes disposable tabs silently, without generating browser
close events. Internal-author messages are not rendered as person bubbles, whether
queued or already injected into a model turn.

After a takeover or person step resolves, the continuation starts with a fresh
`browser_snapshot` of its tab (or `browser_repl`'s `page.snapshot()` when that is
the exposed capture tool), checks whether the requested outcome occurred and
continues remaining steps before replying. Ticket `done` means the person ended
their step, not that login or the task succeeded. Earlier captures and refs remain
invalid; the stale-ref denial is unchanged. The takeover resolution result repeats
this nudge. The HS-6 continuation fixture checks the result, old-ref denial and
fresh page capture.

Before mutating a bulk scope after offering alternatives, a Task restates exactly
which folders, categories or targets are included and excluded. A generic go-ahead
cannot choose among unresolved alternatives; the Task asks which scope was meant.
Counts come from observed rows or explicit tool totals, never estimates. Group
sizes are summed with contributing groups named, and unread and total counts are
kept separate; an unavailable exact count is stated without inventing a number.
The Task policy snapshot is checked by `agent/tests/test_browse_policy.py`.

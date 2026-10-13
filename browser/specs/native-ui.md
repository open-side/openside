# Native Chromium UX and daemon UI entry points (P8–P11)

## P9: use the upstream implementation

Chromium 155 already implements vertical tabs, split view and the bookmarks side
panel. `0006` changes the openside build's default `vertical_tabs.enabled`
preference to `true`, including existing profiles without an explicit value.
Explicit user preferences still win, and a user can turn vertical tabs off.
SplitViewHorizontal is already enabled by default in the pinned Chromium source,
and bookmarks register an ordinary native side-panel entry without a feature
gate. No Helium, Cromite, xplorer or other fork code is copied.

Vertical tabs next to the daemon web UI's own conversation sidebar read as two
sidebars. The `openside-browser` launcher (`tools/openside-browser`; copy it to
`~/.local/bin` beside the `openside` CLI) therefore seeds `vertical_tabs.enabled=false`
into its dedicated profile when that key is absent and the profile is not open; a
user's later toggle in the browser writes the key and is never overwritten. The
patch default stays `true` for other launchers. The same seeding step also sets
`auto_pin_new_tab_groups=false` and `bookmark_bar.show_tab_groups=false` (same
rule: only when absent) so the Agent Tabs group is not duplicated as a pinned chip
in the bookmarks bar.

The authenticated gateway's `browser.status` observes those three native states.
`patched.rs::native_ux_defaults_are_enabled` launches headless with the socket
handshake and asserts all three. It proves availability/defaults, not visual
layout quality or headed keyboard ergonomics.

## P9: glass chrome (`0011`)

The web UI's look (24 dip surfaces, pill controls, translucent glass over the sky
gradient) is applied to the native browser chrome. `0011` changes only values and
one paint call; no new feature flag, preference, or GN argument:

| Surface | Change | Source |
| --- | --- | --- |
| Agent Tabs group created by the gateway | colour `kBlue` -> `kGrey`, matching the extension's group | `chrome/browser/openside/agent_gateway.cc` |
| Grey tab-group chip | fill `#1D1D1F` (light) / `#F5F5F7` (dark) instead of slate grey | `chrome/browser/ui/color/chrome_color_mixer.cc` |
| Tab-group chip | radius 16 (pill) horizontal and vertical; header padding unchanged | `tabs/common/tab_group_style.cc` |
| Vertical tabs | corner radius 16 (pill rows) | `chrome/browser/ui/layout_constants.cc` |
| Vertical tab strip | toolbar-coloured sheet at 0.58 opacity over the frame (theme gradient); replaced by the organizer fade while that panel is open | `frame/layout/browser_view_tabbed_layout_impl.cc` |
| Vertical tab strip scrollbar | `kHiddenButEnabled`: wheel and touch scroll, never drawn | `tabs/common/tab_strip_view.cc` |
| Side panel content | corner radius 8 -> 24 | `chrome/browser/ui/views/chrome_layout_provider.cc` |
| Area around a side panel (header, border) | frame (theme gradient) instead of flat toolbar colour | `frame/main_background_region_view.cc` |

The look comes together with the launcher's theme: `tools/openside-browser` writes
`$XDG_DATA_HOME/openside/browser-theme-{light,dark}` (the web `--backdrop`
gradient as `theme_frame`, `#1d1d1f`/`#f5f5f7` text) and loads it beside the
extension. `OPENSIDE_BROWSER_THEME=light|dark|off` overrides the desktop colour
scheme. Without a theme the strip still turns translucent but shows Chromium's
default frame colour.

Not changed: horizontal tab shape, the main content region radius (8), tab-group
chip colours other than grey, and the native side-panel header controls (title,
pin, close), which stay Chromium's.

## P8: opt-in ordinary daemon web UI

Launch a dedicated patched browser with:

```bash
"$OPENSIDE_PATCHED_BROWSER" \
  --user-data-dir="$(mktemp -d)" \
  --openside-daemon-ui-url="http://127.0.0.1:${OPENSIDE_DAEMON_PORT}/"
```

This switch accepts only an HTTP loopback IP origin (`127.0.0.1` or `[::1]`) with
root path `/`, no username/password, no query and no fragment. An invalid value
is ignored; it never redirects the NTP to a remote origin. No daemon token is
passed through the switch, URL or gateway status.

`0007` makes normal, non-incognito `chrome://newtab/` navigation open that ordinary
web origin. A gateway-enabled profile registers `ask` and `routines` omnibox
keywords through Chromium's TemplateURLService. Existing user keywords win;
there is no replacement of the default search engine. `ask <text>` opens the UI
with a `q` query parameter; the existing web app does not auto-submit that text.
The normal web UI pairing/sign-in still applies. Incognito does not install
keywords or override its NTP.

The minimal permitted fallback is an NTP plus omnibox entry, **not a new
`chrome://openside` privileged WebUI and not a native Ask side panel**. The stock
MV3 extension remains the Ask side-panel path. Native Ask side-panel integration
is BLOCKED in this increment because Chromium's browser-window side-panel view
factory would require an additional UI entry and headed validation beyond this
small NTP integration. **User action:** use the existing extension Ask side panel,
or run a later headed side-panel integration/validation before claiming native
side-panel Ask support.

`patched.rs::daemon_ui_new_tab_and_routines_open_existing_web_ui` serves the real
built web app from a loopback fixture, verifies NTP navigation, observes validated
keyword-presence booleans through the authenticated gateway, opens its existing
routine screen, and checks incognito NTP exclusion. Status never returns stored
templates, and is scoped to the first managed non-incognito profile rather than
aggregating unrelated browser-window profiles. Build `web/dist` before running
it; missing assets are a failure, not a skip or fake UI fallback.

## P11: routines stay daemon-owned

The `routines` keyword targets the existing daemon web UI's `/routines` route.
That page lists routines from `GET /v1/routines` and uses the normal authenticated
web API for creation, pause/resume, run-now and history. Scheduling remains in the
daemon and does not depend on a browser process. The native smoke observes the
real React routine list using loopback API fixture data; it does not claim a
separate C++ scheduler or a privileged browser routine API.

## P10: native secret sink

The [gateway protocol](gateway-protocol.md#11-authenticated-native-status-and-autofill)
defines main-frame field discovery, document-bound one-use handles, and exact
origin checks at both browser and renderer delivery. Native extraction rejects
process-global node IDs belonging to another document and returns the receiving
renderer DocumentToken. `FillFieldForOpenside` checks the actual field document,
that token, and the expected origin immediately before native filling. Handles
also bind to the tab's primary-page generation so a BFCache restoration cannot
revive them. The daemon's Vault login/OTP tools use this sink when a patched
gateway is connected. Stock CDP fallback remains available; native failures are
not downgraded to CDP.

Focused evidence:

- `patched.rs::native_autofill_checks_origin_scope_and_document_without_cdp_values`
- `crates/opensided/src/tests/vault.rs::patched_vault_login_uses_native_autofill_without_cdp_secret_values`
- `patched.rs::native_autofill_refuses_same_renderer_cross_origin_iframe_nodes`
- `patched.rs::native_autofill_handles_stay_revoked_after_bfcache_restore`
- `patched.rs::browser_status_never_returns_sensitive_keywords_or_other_profile_state`

The daemon test creates a real encrypted Vault item, runs the tool, verifies that
the loopback fixture receives submitted username/password canaries, and asserts
that the agent CDP command trace, tool outcomes, durable events and audit contain
neither value. A readonly-field native refusal also proves no CDP fallback.
Post-fill DOM/network visibility to a separately privileged inspector is
not prevented; renderer delivery through native autofill is intentional.

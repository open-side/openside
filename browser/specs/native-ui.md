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

Ctrl+E (⌘E on macOS) toggles the extension's side panel: the manifest suggests it
for `_execute_action`, and with `openPanelOnActionClick` the action toggles the
panel. Chromium keeps Ctrl+E for itself on Linux (focus search), so it never
auto-assigns that key to an extension. The launcher therefore writes the binding
into the profile's `extensions.commands` (`linux:Ctrl+E` or `mac:Command+E`), as
if set in `chrome://extensions/shortcuts`; extension accelerators then take
precedence over the browser's own. It does so once per profile (marker file
`.openside-panel-shortcut`) and only when the extension has no binding for that
command, so a shortcut the person later changes or removes stays that way.
Ctrl+Shift+A (⇧⌘A) is seeded the same way for the extension's `open-palette`
command (`linux:Ctrl+Shift+A` or `mac:Command+Shift+A`, marker
`.openside-palette-shortcut`, so profiles seeded before the palette existed get
it too); see the native shell section below. A key the person already bound to
something else is left as it is.
Preferences are only rewritten when no browser may own the profile: any
`SingletonLock` or `SingletonSocket` (live, stale or another host's) skips the
seeding, the check runs again right before the verified new file is renamed into
place, one launcher seeds at a time, and the markers are written only after that.

The same launcher also pairs the extension with the daemon by itself: it writes a
one-time, extension-origin-bound code into the unpacked extension directory
(`launch-pairing.json`, 0600, not web-accessible, removed after 30 s; installing a
code and removing a launch's own code share one lock, so an older launch's cleanup
never removes a newer launch's code), and the
extension redeems it for a person-scoped token. On every launch it also points the
manifest's `host_permissions` and `externally_connectable.matches` at the running
daemon's exact origin, so no other loopback port or host can message the
extension. Both files go only into the real, user-owned extension directory that
group and others cannot write. A manifest change only takes effect when the
browser starts cold, so the origin each cold start loads is recorded in the
profile (`.openside-extension-origin`). If a browser on this profile is still
running (its `SingletonLock` names this host and a live pid) and loaded another
origin, or was started before that record existed while the manifest just
changed, the launcher hands it nothing and fails with a message asking the person
to quit the browser and open it again. See `docs/specs/remote-ui.md` (owner browser
and local authority) for the security model.

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

Chromium's own UI language follows the OS locale. On Linux it ignores `--lang`
and reads `LANGUAGE`, which counts only when it names a UI locale the browser
ships (`locales/<name>.pak`: `ko`, `pt-BR`, `zh-TW`; not `ko_KR` or `zh_Hant`).
`openside-browser` maps `[browser] language` in config.toml, or
`OPENSIDE_BROWSER_LANG`, which overrides it, onto such a locale and exports it
as `LANGUAGE`. The UI
language also seeds `navigator.languages`, which the web UI and extension follow
while their Language setting is System.

Not changed: horizontal tab shape, the main content region radius (8), tab-group
chip colours other than grey, and the native side-panel header controls (title,
pin, close), which stay Chromium's.

## S20: native shell (`0012`, `0013`)

The openside shell's browser-wide entry points. `0012` adds two Linux/Windows
accelerators and `0013` adds tab renaming; everything else lives in the
extension (`extension/src/shell-commands.ts`) and the web UI.

| Entry point | Key | Implementation |
| --- | --- | --- |
| Split view side by side | Ctrl+Shift+\ (beside Chromium's Alt+Shift+N) | `accelerator_table.cc` maps it to `IDC_NEW_SPLIT_TAB` |
| Split view stacked | Ctrl+Shift+- | new `IDC_OPENSIDE_NEW_SPLIT_TAB_STACKED` (34064) calls `chrome::NewSplitTab` with `SplitTabLayout::kStacked`; enabled exactly where `IDC_NEW_SPLIT_TAB` is (normal windows). It replaces Chromium's duplicate Shift+Ctrl+minus zoom-out; Ctrl+- still zooms out |
| Mini popup | Alt+Space, global, only while Settings › Mini popup is on | extension command `mini-popup` (no suggested key) opens a 480×600 popup window on `<daemon>/mini`; pressed again it brings that window forward, or closes it when it already has focus |
| Search palette | Ctrl+Shift+A (⇧⌘A), seeded by the launcher | extension command `open-palette` activates the window's openside tab (the active one, else the one used last) with `#search`, or opens `<daemon>/#search`; an openside tab with focus handles the key itself |
| Tab context menu | — | Chromium 155 already offers Aside's tab items; the extension adds "Add Tab to Bookmarks" (filed in the bookmarks bar), and `0013` adds Rename (below) |
| Rename a tab | tab context menu › Rename tab… (single tab) | `0013`, below |

Like other non-reserved browser accelerators, the split keys reach a page first
and act only when the page does not consume them.

The mini popup toggle is the person gateway preference `mini_popup`
([gateway-protocol.md](gateway-protocol.md) §12): on binds Alt+Space to the
extension's command as a global shortcut, off removes the binding, and it reads
`null` when the extension is not enabled. Turning it on never takes Alt+Space
from another extension: the request fails as `unsupported` and the setting
stays off. Chromium never auto-assigns a global
key other than Ctrl+Shift+digit, and a binding seeded by the launcher would take
Alt+Space system-wide while the setting reads off, so it is only ever written by
this toggle (or by the person in `chrome://extensions/shortcuts`, which is also
how a stock browser gets it). `patched.rs::browser_prefs_bind_the_mini_popup_shortcut_of_the_openside_extension`
checks the round trip, and the refusal when another extension holds the key,
against what `chrome://extensions/shortcuts` shows.

Tab renaming (`0013`). The tab context menu shows "Rename tab…" right after
Duplicate (Aside's place for Rename) when one tab is selected, and "Reset tab
name" under it while that tab has a name. Rename opens a bubble anchored on the
tab (below a horizontal tab, beside a vertical one; `openside_tab_rename_bubble`)
with one field prefilled with the current title and selected. Enter or closing
the bubble by clicking elsewhere stores the edited text; Esc, or leaving the
text untouched, changes nothing; an empty text removes the name. Closing the
tab (or its window) while the bubble is open closes the bubble and stores
nothing. Whitespace runs collapse to one space, the name is cut at 256
characters (grapheme-safe: counted with `BreakIterator`, so a surrogate pair,
combining sequence or emoji sequence is never split), and trailing whitespace
left by the cut is trimmed.
The name lives in `TabUIHelper` (`SetCustomTitle`), so the horizontal and
vertical tab strips, the tooltip, the hover card and tab search show it in
place of the page title. It belongs to the origin of the committed document
(its tuple, or its precursor's), and survives page title changes and
navigations that commit the same origin: a sandboxed page or an about:blank the
page opens itself counts as the origin it came from, and an error page as the
URL it failed to load. It is dropped when the tab's main frame commits another
origin, including an about:blank the browser loads (typed, or opened by an
agent), which has no origin, and by Reset. A name given on a page with no tuple
origin (a new blank tab) shows, is not saved, and the next commit drops it.
Only names on http and https origins are saved with the session (restore
applies the same rule); a name on a file: page or another non-http origin shows
and follows the same origin rules but does not come back after a restart. A
saved name writes `SessionService::AddTabExtraData` under `openside_custom_title`
on every set, clear or origin drop, with the value `<http(s) origin>\n<name>`
(empty once cleared), and `BuildCommandsForTab`
re-emits it because a session rebuild keeps no extra data otherwise. Restore
(`CreateRestoredTab`) checks the record before stashing it on the contents: the
origin must be a valid http or https tuple origin that serializes back to the same text and
matches the origin of the entry the tab is about to load, and the name must be
one line of UTF-8 that is not empty after the same normalization and cap as a
typed name; anything else restores no name. The tab's `TabUIHelper` then takes
it back with the stored origin, so the cross-origin rule holds before the
restored page commits. Reopening a closed tab (Ctrl+Shift+T) does not bring the
name back: `TabRestoreService` does not record extra data from live tabs. The
page, `WebContents::GetTitle()`, CDP targets, extension tab APIs and the
gateway's tab records keep the page title. The menu strings come from
`chrome/browser/ui/tabs/openside_tab_rename_strings.h`, keyed by the UI locale
(English and Korean), not from the grd. Command ids are
`CommandOpensideRenameTab` (29) and `CommandOpensideResetTabName` (30),
appended after Chromium's own so its histogram values keep their meaning.

Test hook: with `--openside-test-tab-rename`, the agent gateway also serves
`test.tab_rename` with `{tab_id, action, text, key, url}` on any tab in a
vertical tab strip, driving the same path a person does. `open_editor` and
`reset` activate the item in a real `TabMenuModel` run by
`TabContextMenuController` over the window's tab strip controller (an absent or
disabled item is `unavailable`); `edit` replaces the open bubble's text as typing
over its selection does (`text` optional), then presses Enter or Esc on the
field (`key: "enter" | "escape"`) or closes the bubble as losing focus does
(`key: "blur"`); `navigate` loads `url` in the tab; `restore` clears the name
and runs the hex-encoded record `extra_hex` through the restore path against the
tab's current page; `get` changes nothing. Every
answer is `{title, custom, saved, menu, reset_enabled, editor}`: the title the tab
strip renders (`TabData`), whether the tab has a name, whether that name would be
written to the session, the tab context menu's
top-level command ids without separators, whether Reset is enabled, and the
open bubble's text (null without one). Without the switch the method is
`invalid_request` (unknown type). `tools/openside-browser` drops any
`--openside-test-*` switch a person passes to it, with a warning on stderr.
`patched.rs::tab_rename_survives_same_origin_and_clears_cross_origin` and
`patched.rs::tab_rename_survives_a_restart_with_session_restore` use it.

Not built (D6): a global menu bar (Linux has none; Downloads, Extensions and
Task Manager are in Chromium's app menu), an OS notification permission prompt
on Linux (the extension holds `notifications`), Ctrl+Shift+C to copy the URL
(DevTools inspect owns it on Linux), and the macOS Window menu and macOS split
keys. The palette on a tab that is not openside's switches to the openside tab
instead of drawing an overlay over the page.

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

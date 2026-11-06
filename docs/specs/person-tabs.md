# The person's own browser tabs

openside reads and drives the person's own browser (the one they browse in, not the agent's
headless browser) through the openside extension running in it. That browser has no debugging
port, so the extension keeps a WebSocket open to the daemon:

- `GET /v1/browser/person/bridge`: WebSocket; only a person-device token presented with the
  extension's origin (`chrome-extension://jneddlbpgmlheplolfonafkbkbkjhcfj`) may connect. The
  extension sends `{kind:"hello",protocol:"openside.person-browser",version:1,run}` first; `run` is
  random per browser run. The daemon sends `{id,method,params}` and the extension answers
  `{id,result}` or `{id,error:{code}}`. The daemon pings every 20 s so the MV3 worker stays alive;
  a newer connection from the same paired device replaces the older one, while a different
  device is refused as long as the current link is live. Calls time out after 10 s. Like every
  person route, the bridge trusts whoever holds the person-device token: a copy of the token in
  another browser profile counts as the same device and may replace the link (revoke it in
  Settings › Devices).
- Methods: `tabs.list`, `tabs.command`, `sessions.recently_closed`, `sessions.restore`,
  `history.search`, `bookmarks.tree`, `bookmarks.create`, `bookmarks.remove`, `downloads.list`,
  `extensions.list`, `tab.snapshot`, `tab.act`.

## Person routes

The web shell lists and manages tabs, groups, recently closed tabs, history, bookmarks,
downloads and extensions through `/v1/browser/person/*` (see `docs/reference/api.md`). Agent
clients (MCP/CLI tokens, marked socket clients) get `403 person_device_required`. Reads answer
`connected:false` with empty lists when no extension is connected; mutations answer
`409 person_browser_not_connected`. The local socket is the same-user trust boundary, as for
every other person route: the client marker only keeps well-behaved agent clients out.

## Attaching a tab to a chat

`POST /v1/sessions/{id}/attached-tabs` (the `@` mention or Ask on this tab) attaches one tab,
or the selected tab of the focused window. Attachments live in memory for the browser run: a new
`run` drops them, since Chrome reuses tab ids after a restart. Deleting the chat drops them too.

## Agent tools

| Tool | Policy | Notes |
|---|---|---|
| `browser_user_tabs` | `privacy_read` `person-browser/tabs` | Titles and URLs (no query) of non-incognito tabs; untrusted. |
| `browser_user_tab_attach` | `privacy_read` `person-browser/tabs` | Attaches an ordinary web page (`attached_by: agent`). |
| `browser_user_tab_snapshot` | `browse` on the tab's origin | Attached tabs only; outline with `uN` refs; field values (and the text of editable fields) never read. |
| `browser_user_tab_act` | `person_browser` on the tab's origin (+ `sensitive`, + `browse` for navigate) | Attached tabs only; Guard asks (unless the person's own Allow rule matches, as for every capability), Full allows, Read-only denies. |

The extension refuses incognito tabs, browser and extension pages, and the openside UI itself
(the paired daemon's port under any loopback alias; the daemon refuses those URLs too), and
re-checks that the tab is still on the origin the daemon's policy judged (`origin_changed`):
before injecting, inside the injected action, and on the snapshot's own URL afterwards, so a
navigation during the injection neither acts on nor returns another site.
A ref whose element was relabelled since the snapshot answers `stale_ref`, so the `sensitive`
judgement made from the snapshot label still holds. A submit (`type` with `submit`), an Enter
press, or a key pressed without a ref always counts as `sensitive`. A chat keeps at most 20
attached tabs; a tab the person closed leaves every chat on the next tab list or when the
browser answers `tab_not_found`. A site the extension cannot access yet answers `site_access_required`; the
extension opens `site-access.html` next to the tab (at most once a minute per site, and at
most three pages a minute across all sites), whose
Allow button requests that one site (`optional_host_permissions`; Chrome only grants it from a
click). The agent is told to ask the person and retry. Only a grant naming the site's host
counts: a wildcard host permission the person gave another feature (e.g. `<all_urls>` for
Lasso) does not let agents read or act on a site.

The extension holds `sessions`, `downloads` and `management` for the shell's recently closed,
downloads and extensions views (read-only except restore), `browsingData` for
`POST /v1/browser/person/clear-data` (the person's Delete browsing data dialog: chosen kinds
since a time, audited without the data), and asks for site access per site.

Person routes return full URLs: they serve the person's own shell, which shows what their
browser shows. Agent-facing tools strip query and fragment (`public_url`), and the audit log
records no URLs.
